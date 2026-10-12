use std::{
    collections::HashSet,
    fs, io, iter,
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
    os::unix::fs::MetadataExt,
    time::Instant,
};

use oblivion_one::compositor::{
    CompositorFrameBatchId, DirectScanoutFeedbackCapabilities, DirectScanoutFormatCapability,
    DmabufKmsPreferredState, DrmContentType, FrameBatchDiscardReason, OwnCompositorServer,
    SurfaceDamagePresentation, SurfacePipelineEvent,
};
use oblivion_one::native::buffering::O1AdmissionObservation;
use oblivion_one::native::buffering::PresentationOpportunity;
use oblivion_one::native::kms::{AtomicDiscovery, DrmFormatModifierPair, FramebufferId};
use oblivion_one::native::presentation_deadline::{
    MonotonicTimestampNs, PresentationTarget, PrimaryRefreshClaim,
};
use oblivion_one::render_backend::{
    buffer::{DrmFormat, DrmModifier},
    egl_gles::EglGlesDmabufFormat,
};

use crate::egl_renderer::native_fence::NativeRenderFence;
use crate::egl_renderer::{
    EglSceneFrameCommit, FrameSkipReason, GlesSceneFrameStats, OutputDamage,
};
use crate::native_output::runtime::take_client_cursor_presentation_feedback_batch;
use crate::native_output::runtime::{
    DirectCallbackLeakMetrics, DirectTerminalCallbackDisposition, DmabufGpuReleaseSafety,
    FrozenNativeRenderFrame, ResolvedNativeFrameScene, direct_terminal_callback_owner_leaks,
    discard_presentation_feedback_obligation, restore_presentation_feedback_obligation,
    settle_failed_output_transaction, settle_no_visual_change_output_transaction,
};
use oblivion_one::window_lifecycle_animation::LifecycleRenderFallbacks;

use super::atomic_direct::{direct_candidate_key, direct_scanout_debug};
use super::atomic_render_domain::{
    AtomicRenderDomain, AtomicRenderDomainBootstrap, AtomicRenderDomainInfo,
};
pub(crate) use super::atomic_render_domain::{AtomicRenderedFrameParts, AtomicSlotRenderOutcome};
use super::render_worker::{
    AtomicRenderCandidateIdentity, AtomicRenderCandidateJob, AtomicRenderControl,
    AtomicRenderTransportKind, AtomicRenderWorkerCapture, AtomicRenderWorkerEffectReload,
    AtomicRenderWorkerEvent, AtomicRenderWorkerHandle, AtomicRenderWorkerIdentity,
    AtomicRenderWorkerInitialFrame, AtomicRenderWorkerJob, AtomicRenderWorkerPolicy,
    AtomicRenderWorkerReleaseFence, AtomicRenderWorkerStartup, RenderWorkerJobId,
    SequencedRenderControl, apply_render_control,
};
use super::*;
use crate::native_output::presentation::async_validation::CompositedPresentationValidationKey;
use crate::native_output::presentation::kms_timing::{KmsPresentationTiming, KmsSubmitWindow};
use crate::native_output::presentation::transaction::{
    FramePresentationReservation, O1PrepareIntent, OutputPresentationStateKey,
    VariablePresentationReservation, kms_presentation_state_is_represented,
};
use oblivion_one::native::presentation_deadline::PresentationDeadlinePlanner;
use oblivion_one::native::presentation_timing::PresentationTiming;
use oblivion_one::native::vrr_window::{VrrWindowPlanInput, VrrWindowPlanner};

fn elapsed_nanos(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn teardown_render_before_physical(
    destroy_render_domain: impl FnOnce(),
    release_physical_pool: impl FnOnce(),
) {
    destroy_render_domain();
    release_physical_pool();
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct AtomicRenderPhaseDurations {
    pub(crate) egl_context_activation_ns: u64,
    pub(crate) render_target_preparation_ns: u64,
    pub(crate) render_snapshot_freeze_ns: u64,
    pub(crate) scene_drawing_ns: u64,
    pub(crate) native_render_fence_export_ns: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct VariablePageflipQualification {
    rejected_output_generation: Option<u64>,
}

impl VariablePageflipQualification {
    fn reject_generation(&mut self, output_generation: u64) {
        self.rejected_output_generation = Some(output_generation);
    }

    fn rejects_mode(self, mode: OutputPresentationMode, output_generation: u64) -> bool {
        mode.uses_vrr() && self.rejected_output_generation == Some(output_generation)
    }

    fn clear(&mut self) {
        self.rejected_output_generation = None;
    }
}

#[cfg(test)]
mod variable_pageflip_qualification_tests {
    use super::*;

    #[test]
    fn early_variable_pageflip_rejects_vrr_for_its_output_generation() {
        let mut qualification = VariablePageflipQualification::default();
        qualification.reject_generation(7);

        assert!(qualification.rejects_mode(OutputPresentationMode::AdaptiveSync, 7));
        assert!(qualification.rejects_mode(OutputPresentationMode::AdaptiveAsync, 7));
        assert!(!qualification.rejects_mode(OutputPresentationMode::AdaptiveSync, 8));
        assert!(!qualification.rejects_mode(OutputPresentationMode::Vsync, 7));

        qualification.clear();
        assert!(!qualification.rejects_mode(OutputPresentationMode::AdaptiveSync, 7));
    }
}

#[cfg(test)]
mod confirmed_pageflip_tests;
mod direct;
mod worker;

#[path = "atomic_egl_gbm_transactions.rs"]
mod atomic_egl_gbm_transactions;

pub(crate) struct AtomicEglGbmScanout {
    _physical_device: Option<gbm::Device<std::os::fd::OwnedFd>>,
    physical_pool: Option<AtomicPhysicalOutputPool>,
    render_transport: AtomicRenderTransport,
    pending_render_controls: Vec<SequencedRenderControl>,
    render_control_epoch: u64,
    render_control_overflowed: bool,
    render_slot_identities: Vec<(OutputSlotId, u64)>,
    lifecycle_animation_available: bool,
    squash_animation_available: bool,
    swapchain: Option<AtomicOutputSwapchain>,
    direct: DirectScanoutControl,
    width: u32,
    height: u32,
    renderable_dmabuf_formats: Vec<EglGlesDmabufFormat>,
    dmabuf_feedback: EglGlesDmabufFeedback,
    dmabuf_main_device: Option<u64>,
    dmabuf_main_device_path: Option<String>,
    dmabuf_egl_vendor: String,
    dmabuf_scanout_capabilities: DirectScanoutFeedbackCapabilities,
    dmabuf_kms_preferred_state: DmabufKmsPreferredState,
    pub(crate) format_modifier: DrmFormatModifierPair,
    drm_cleanup_armed: bool,
    deadline_hints_enabled: bool,
    counters: ExplicitOutputCounters,
    async_page_flip_capable: bool,
    async_format_capable: bool,
    vrr_connector_capable: bool,
    vrr_crtc_property_available: bool,
    connector_content_types: HashSet<DrmContentType>,
    async_crtc_id: u32,
    async_primary_plane_id: u32,
    async_output_generation: u64,
    presentation_validation_accepted: HashSet<CompositedPresentationValidationKey>,
    presentation_validation_rejected: HashSet<CompositedPresentationValidationKey>,
    variable_pageflip_qualification: VariablePageflipQualification,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AtomicAsyncPolicyInputs {
    pub(crate) cursor_transition_pending: bool,
    pub(crate) kms_lane_free: bool,
    pub(crate) confirmed_presentation_state: OutputPresentationStateKey,
    pub(crate) pending_presentation_state: Option<OutputPresentationStateKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PreparedCompositedPresentationState {
    pub(crate) mode: OutputPresentationMode,
    pub(crate) content_type: DrmContentType,
    pub(crate) validation_key: Option<CompositedPresentationValidationKey>,
    pub(crate) validation_accepted: bool,
    pub(crate) confirmed_state_matches: bool,
    pub(crate) presentation_state_transition_required: bool,
    vrr_blocker: Option<VrrBlocker>,
    async_blocker: Option<AsyncBlocker>,
    combination_blocker: Option<&'static str>,
    acquire_strategy: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreparedCompositedReservation {
    Fixed {
        target: PresentationTarget,
        submit_window: KmsSubmitWindow,
    },
    Variable {
        reservation: VariablePresentationReservation,
    },
}

impl PreparedCompositedReservation {
    pub(crate) const fn frame_reservation(self) -> FramePresentationReservation {
        match self {
            Self::Fixed { target, .. } => FramePresentationReservation::Fixed(target),
            Self::Variable { reservation } => FramePresentationReservation::Variable(reservation),
        }
    }

    pub(crate) const fn presentation_timing(
        self,
        rendered_at: MonotonicTimestampNs,
    ) -> KmsPresentationTiming {
        match self {
            Self::Fixed { submit_window, .. } => KmsPresentationTiming::Fixed(submit_window),
            Self::Variable { .. } => KmsPresentationTiming::variable(rendered_at.get()),
        }
    }

    pub(crate) const fn fixed_target(self) -> Option<PresentationTarget> {
        match self {
            Self::Fixed { target, .. } => Some(target),
            Self::Variable { .. } => None,
        }
    }

    pub(crate) const fn fixed_submit_window(self) -> Option<KmsSubmitWindow> {
        match self {
            Self::Fixed { submit_window, .. } => Some(submit_window),
            Self::Variable { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VariableCompositedAdmission {
    pub(crate) reactive_double: bool,
    pub(crate) desired_credit_one: bool,
    pub(crate) render_ahead: bool,
    pub(crate) fixed_target_owned: bool,
    pub(crate) commit_timing_applies: bool,
    pub(crate) deferred_o1: bool,
}

pub(crate) fn plan_composited_variable_reservation(
    state: PreparedCompositedPresentationState,
    admission: VariableCompositedAdmission,
    deadline: &PresentationDeadlinePlanner,
    anchor: Option<MonotonicTimestampNs>,
    now: MonotonicTimestampNs,
    shortest_interval: Duration,
) -> Option<VariablePresentationReservation> {
    if state.mode != OutputPresentationMode::AdaptiveSync
        || state.validation_key.is_none()
        || !state.validation_accepted
        || !state.confirmed_state_matches
        || state.presentation_state_transition_required
        || !admission.reactive_double
        || !admission.desired_credit_one
        || admission.render_ahead
        || admission.fixed_target_owned
        || admission.commit_timing_applies
        || admission.deferred_o1
    {
        return None;
    }
    let anchor = anchor?;
    let opportunity_id = deadline.next_variable_opportunity_id()?;
    let timing = VrrWindowPlanner::plan(VrrWindowPlanInput {
        anchor,
        now,
        not_before: None,
        shortest_interval,
    })?;
    if !matches!(timing, PresentationTiming::VrrWindow { .. }) {
        return None;
    }
    VariablePresentationReservation::try_new(
        PresentationOpportunity::new(opportunity_id, timing),
        oblivion_one::native::presentation_deadline::PresentationTargetReason::ReactiveDouble,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeferredO1BindingResult {
    NotReady,
    WaitingForPredecessor,
    Bound { advanced_intervals: u64 },
    Stale(DeferredO1BindingFailure),
}

fn damage_for_physical_presentation_state(
    damage: &crate::native_output::NativeOutputDamage,
    presentation_state_transition_required: bool,
    output_width: u32,
    output_height: u32,
) -> crate::native_output::NativeOutputDamage {
    if damage.is_empty() && presentation_state_transition_required {
        crate::native_output::NativeOutputDamage::full_output(output_width, output_height)
    } else {
        damage.clone()
    }
}

impl AtomicAsyncPolicyInputs {
    pub(crate) const fn new(
        cursor_transition_pending: bool,
        kms_lane_free: bool,
        confirmed_presentation_state: OutputPresentationStateKey,
        pending_presentation_state: Option<OutputPresentationStateKey>,
    ) -> Self {
        Self {
            cursor_transition_pending,
            kms_lane_free,
            confirmed_presentation_state,
            pending_presentation_state,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ExplicitOutputCounters {
    pub(crate) sync_file_deadline_hints_applied: u64,
    pub(crate) sync_file_deadline_hints_unsupported: u64,
    pub(crate) sync_file_deadline_hints_failed: u64,
    pub(crate) atomic_submissions: u64,
    pub(crate) atomic_in_fence_submissions: u64,
    pub(crate) async_userspace_fence_submissions: u64,
    pub(crate) atomic_out_fences_received: u64,
    pub(crate) atomic_out_fence_missing: u64,
    pub(crate) render_fence_timing_unavailable: u64,
}

impl ExplicitOutputCounters {
    pub(crate) fn note_atomic_submission(
        &mut self,
        presentation_mode: oblivion_one::compositor::OutputPresentationMode,
    ) {
        self.atomic_submissions += 1;
        if presentation_mode.is_async() {
            self.async_userspace_fence_submissions += 1;
        } else {
            self.atomic_in_fence_submissions += 1;
        }
    }
}

struct DeviceAllocationProbe<'a> {
    device: &'a gbm::Device<std::os::fd::OwnedFd>,
    width: u32,
    height: u32,
}

fn discover_direct_scanout_capabilities(
    plane_formats: &[DrmFormatModifierPair],
    renderer_dmabuf_feedback: &EglGlesDmabufFeedback,
) -> Vec<DirectScanoutFormatCapability> {
    plane_formats
        .iter()
        .copied()
        .filter_map(|format| {
            let drm_format = DrmFormat::from_fourcc(format.fourcc);
            if !drm_format.is_opaque_rgb8888() || format.modifier == DrmModifier::INVALID.0 {
                return None;
            }
            let modifier = DrmModifier(format.modifier);
            if !renderer_dmabuf_feedback.supports(drm_format, modifier) {
                return None;
            }
            Some(DirectScanoutFormatCapability {
                format: format.fourcc,
                modifier: format.modifier,
            })
        })
        .collect()
}

impl GbmAllocationProbe for DeviceAllocationProbe<'_> {
    fn supports(&mut self, candidate: DrmFormatModifierPair) -> bool {
        let Ok(format) = gbm::Format::try_from(candidate.fourcc) else {
            return false;
        };
        self.device
            .create_buffer_object_with_modifiers2::<()>(
                self.width,
                self.height,
                format,
                iter::once(gbm::Modifier::from(candidate.modifier)),
                gbm::BufferObjectFlags::SCANOUT | gbm::BufferObjectFlags::RENDERING,
            )
            .is_ok()
    }
}

fn create_physical_output_pool(
    device: &gbm::Device<std::os::fd::OwnedFd>,
    drm: BorrowedFd<'_>,
    width: u32,
    height: u32,
    format_modifier: DrmFormatModifierPair,
    pool_generation: u64,
) -> io::Result<AtomicPhysicalOutputPool> {
    let format = gbm::Format::try_from(format_modifier.fourcc)
        .map_err(|_| io::Error::other("selected output FourCC is unsupported by GBM"))?;
    let usage = gbm::BufferObjectFlags::SCANOUT | gbm::BufferObjectFlags::RENDERING;
    let mut physical_slots = Vec::with_capacity(EXPLICIT_OUTPUT_SLOT_CAPACITY);

    for raw_id in 0..EXPLICIT_OUTPUT_SLOT_CAPACITY {
        let mut framebuffer_created = None;
        let slot = (|| {
            let bo = device.create_buffer_object_with_modifiers2::<()>(
                width,
                height,
                format,
                iter::once(gbm::Modifier::from(format_modifier.modifier)),
                usage,
            )?;
            let descriptor = explicit_framebuffer_descriptor(&bo)?;
            let framebuffer = add_explicit_framebuffer(drm, &descriptor)?;
            framebuffer_created = Some(framebuffer);
            let id = OutputSlotId::new(u8::try_from(raw_id).unwrap()).unwrap();
            let physical = AtomicOutputSlotBacking::new(id, pool_generation, bo, framebuffer);
            Ok(physical)
        })();

        match slot {
            Ok(physical) => physical_slots.push(physical),
            Err(error) => {
                if let Some(framebuffer) = framebuffer_created {
                    let _ = drm_ffi::mode::rm_fb(drm, framebuffer.get());
                }
                AtomicPhysicalOutputPool::remove_framebuffers(&physical_slots, drm);
                drop(physical_slots);
                return Err(error);
            }
        }
    }

    if let Err(error) = AtomicPhysicalOutputPool::validate_slots(&physical_slots, pool_generation) {
        AtomicPhysicalOutputPool::remove_framebuffers(&physical_slots, drm);
        drop(physical_slots);
        return Err(error);
    }
    let physical_slots: [AtomicOutputSlotBacking; EXPLICIT_OUTPUT_SLOT_CAPACITY] =
        match physical_slots.try_into() {
            Ok(slots) => slots,
            Err(slots) => {
                AtomicPhysicalOutputPool::remove_framebuffers(&slots, drm);
                drop(slots);
                return Err(io::Error::other(
                    "explicit physical output pool did not construct 3 slots",
                ));
            }
        };
    Ok(AtomicPhysicalOutputPool::from_validated_slots(
        physical_slots,
        pool_generation,
    ))
}

fn create_physical_pool_with_render_import(
    device: &gbm::Device<std::os::fd::OwnedFd>,
    drm: BorrowedFd<'_>,
    width: u32,
    height: u32,
    format_modifier: DrmFormatModifierPair,
    pool_generation: u64,
    import: impl FnOnce(Vec<AtomicRenderSlotDescriptor>, &[(OutputSlotId, u64)]) -> io::Result<()>,
) -> io::Result<AtomicPhysicalOutputPool> {
    let mut physical_pool = Some(create_physical_output_pool(
        device,
        drm,
        width,
        height,
        format_modifier,
        pool_generation,
    )?);
    let physical_identities = physical_pool
        .as_ref()
        .expect("new physical pool remains owned during render import")
        .identities();
    let descriptors = match physical_pool
        .as_ref()
        .expect("new physical pool remains owned during descriptor export")
        .export_render_slot_descriptors()
    {
        Ok(descriptors) => descriptors,
        Err(error) => {
            if let Some(pool) = physical_pool.take() {
                pool.teardown(drm, true);
            }
            return Err(error);
        }
    };
    if let Err(error) = import(descriptors, &physical_identities) {
        if let Some(pool) = physical_pool.take() {
            pool.teardown(drm, true);
        }
        return Err(error);
    }
    Ok(physical_pool.expect("successful import retains physical pool"))
}

enum AtomicRenderTransport {
    Synchronous(AtomicRenderDomain),
    Worker(AtomicRenderWorkerHandle),
}

/// Reactor-owned physical half of an output-configuration candidate. Its BOs
/// remain alive until the render worker acknowledges candidate destruction or
/// promotion of the matching render domain.
pub(crate) struct AtomicOutputConfigurationCandidate {
    identity: AtomicRenderCandidateIdentity,
    physical_device: Option<gbm::Device<OwnedFd>>,
    physical_pool: Option<AtomicPhysicalOutputPool>,
    discovery: Option<AtomicDiscovery>,
    render_domain_may_own: bool,
    drm_rdev: u64,
    format_modifier: DrmFormatModifierPair,
    width: u32,
    height: u32,
}

impl AtomicOutputConfigurationCandidate {
    pub(crate) const fn identity(&self) -> AtomicRenderCandidateIdentity {
        self.identity
    }

    pub(crate) const fn pool_generation(&self) -> u64 {
        self.identity.pool_generation
    }

    pub(crate) fn worker_domain_destroyed(&mut self) {
        self.render_domain_may_own = false;
    }

    pub(crate) fn framebuffer(&self, slot: OutputSlotId) -> io::Result<FramebufferId> {
        self.physical_pool
            .as_ref()
            .ok_or_else(|| io::Error::other("candidate physical pool is unavailable"))?
            .physical_slot(slot)
            .map(|slot| slot.framebuffer)
    }

    fn take_parts(
        &mut self,
    ) -> io::Result<(
        gbm::Device<OwnedFd>,
        AtomicPhysicalOutputPool,
        AtomicDiscovery,
        u64,
        DrmFormatModifierPair,
        u32,
        u32,
    )> {
        if self.physical_device.is_none()
            || self.physical_pool.is_none()
            || self.discovery.is_none()
        {
            return Err(io::Error::other(
                "candidate physical ownership was already transferred",
            ));
        }
        Ok((
            self.physical_device
                .take()
                .expect("candidate device checked"),
            self.physical_pool.take().expect("candidate pool checked"),
            self.discovery.take().expect("candidate discovery checked"),
            self.drm_rdev,
            self.format_modifier,
            self.width,
            self.height,
        ))
    }
}

impl Drop for AtomicOutputConfigurationCandidate {
    fn drop(&mut self) {
        if self.render_domain_may_own {
            // The worker may still have an EGLImage referencing this BO. A
            // fatal path retains it until process exit rather than risking GPU
            // use-after-free.
            if let Some(pool) = self.physical_pool.take() {
                std::mem::forget(pool);
            }
            if let Some(device) = self.physical_device.take() {
                std::mem::forget(device);
            }
        } else {
            if let (Some(pool), Some(device)) =
                (self.physical_pool.take(), self.physical_device.as_ref())
            {
                pool.teardown(device.as_fd(), true);
            }
            self.physical_device.take();
        }
    }
}

const MAX_PENDING_RENDER_CONTROLS: usize = 16;

fn create_synchronous_render_setup(
    kms: &fs::File,
    discovery: &AtomicDiscovery,
    physical_device: &gbm::Device<std::os::fd::OwnedFd>,
    width: u32,
    height: u32,
    pool_generation: u64,
) -> io::Result<(
    AtomicRenderTransport,
    DrmFormatModifierPair,
    super::atomic_render_domain::AtomicRenderDomainInfo,
    AtomicPhysicalOutputPool,
)> {
    let render_fd = duplicate_fd_cloexec(kms.as_raw_fd()).map_err(io::Error::from_raw_os_error)?;
    let render_device = gbm::Device::new(render_fd)?;
    let bootstrap = AtomicRenderDomainBootstrap::new(render_device)?;
    let mut probe = DeviceAllocationProbe {
        device: physical_device,
        width,
        height,
    };
    let format_modifier = select_output_format_modifier_for_presentation(
        oblivion_one::compositor::OutputPresentationMode::Vsync,
        &discovery.plane_scanout_formats,
        Some(&discovery.plane_async_scanout_formats),
        bootstrap.renderable_dmabuf_formats(),
        &mut probe,
    )?;
    let (mut domain, info) = bootstrap.initialize_renderer(width, height, format_modifier)?;
    let pool = create_physical_pool_with_render_import(
        physical_device,
        kms.as_fd(),
        width,
        height,
        format_modifier,
        pool_generation,
        |descriptors, identities| domain.install_pool(descriptors, pool_generation, identities),
    )?;
    Ok((
        AtomicRenderTransport::Synchronous(domain),
        format_modifier,
        info,
        pool,
    ))
}

fn create_worker_render_setup(
    kms: &fs::File,
    discovery: &AtomicDiscovery,
    physical_device: &gbm::Device<std::os::fd::OwnedFd>,
    width: u32,
    height: u32,
    pool_generation: u64,
    initial_generation: std::sync::Arc<oblivion_one::effects::EffectRegistryGeneration>,
    cursor_image: std::sync::Arc<oblivion_one::cursor_theme::CompositorCursorImage>,
) -> io::Result<(
    AtomicRenderTransport,
    DrmFormatModifierPair,
    super::atomic_render_domain::AtomicRenderDomainInfo,
    AtomicPhysicalOutputPool,
)> {
    let render_device_fd =
        duplicate_fd_cloexec(kms.as_raw_fd()).map_err(io::Error::from_raw_os_error)?;
    let (mut worker, renderable_formats) = AtomicRenderWorkerHandle::start(render_device_fd)?;
    let mut probe = DeviceAllocationProbe {
        device: physical_device,
        width,
        height,
    };
    let format_modifier = select_output_format_modifier_for_presentation(
        oblivion_one::compositor::OutputPresentationMode::Vsync,
        &discovery.plane_scanout_formats,
        Some(&discovery.plane_async_scanout_formats),
        &renderable_formats,
        &mut probe,
    )?;
    let mut render_info = None;
    let pool = create_physical_pool_with_render_import(
        physical_device,
        kms.as_fd(),
        width,
        height,
        format_modifier,
        pool_generation,
        |descriptors, identities| {
            render_info = Some(worker.initialize(
                width,
                height,
                pool_generation,
                format_modifier,
                descriptors,
                identities.to_vec(),
                std::sync::Arc::clone(&initial_generation),
                std::sync::Arc::clone(&initial_generation),
                std::sync::Arc::clone(&cursor_image),
            )?);
            Ok(())
        },
    )?;
    let render_info = render_info.expect("successful worker import returns renderer info");
    Ok((
        AtomicRenderTransport::Worker(worker),
        format_modifier,
        render_info,
        pool,
    ))
}

impl AtomicEglGbmScanout {
    fn queue_render_control(&mut self, control: AtomicRenderControl) -> io::Result<()> {
        match &mut self.render_transport {
            AtomicRenderTransport::Synchronous(domain) => apply_render_control(domain, control),
            AtomicRenderTransport::Worker(worker) => {
                if self.pending_render_controls.len() >= MAX_PENDING_RENDER_CONTROLS {
                    self.render_control_overflowed = true;
                    worker.fail_control_lane();
                    return Err(io::Error::other(
                        "bounded renderer-control lane is full; refusing to drop renderer state",
                    ));
                }
                let Some(next_epoch) = self.render_control_epoch.checked_add(1) else {
                    self.render_control_overflowed = true;
                    worker.fail_control_lane();
                    return Err(io::Error::other("renderer control epoch exhausted"));
                };
                self.render_control_epoch = next_epoch;
                self.pending_render_controls.push(SequencedRenderControl {
                    epoch: self.render_control_epoch,
                    control,
                });
                Ok(())
            }
        }
    }

    fn take_render_controls_for_job(&mut self) -> io::Result<(Vec<SequencedRenderControl>, u64)> {
        if self.render_control_overflowed {
            return Err(io::Error::other(
                "renderer-control lane overflowed before worker admission",
            ));
        }
        Ok((
            std::mem::take(&mut self.pending_render_controls),
            self.render_control_epoch,
        ))
    }

    fn render_worker(&self) -> Option<&AtomicRenderWorkerHandle> {
        match &self.render_transport {
            AtomicRenderTransport::Synchronous(_) => None,
            AtomicRenderTransport::Worker(worker) => Some(worker),
        }
    }

    fn render_worker_mut(&mut self) -> Option<&mut AtomicRenderWorkerHandle> {
        match &mut self.render_transport {
            AtomicRenderTransport::Synchronous(_) => None,
            AtomicRenderTransport::Worker(worker) => Some(worker),
        }
    }

    pub(crate) fn render_worker_event_fd(&self) -> Option<i32> {
        self.render_worker().map(AtomicRenderWorkerHandle::event_fd)
    }

    pub(crate) fn render_worker_busy(&self) -> bool {
        self.render_worker()
            .is_some_and(AtomicRenderWorkerHandle::busy)
    }

    pub(crate) fn render_worker_active(&self) -> bool {
        matches!(&self.render_transport, AtomicRenderTransport::Worker(_))
    }

    pub(crate) fn render_target_request(
        &self,
        slot: OutputSlotId,
    ) -> io::Result<RenderTargetRequest> {
        let presentation_serial = self
            .swapchain
            .as_ref()
            .map_or(0, AtomicOutputSwapchain::presentation_serial);
        self.physical_pool
            .as_ref()
            .ok_or_else(|| io::Error::other("explicit physical output pool is unavailable"))?
            .render_target_request(slot, presentation_serial)
    }

    pub(crate) fn try_take_render_worker_event(
        &mut self,
    ) -> io::Result<Option<AtomicRenderWorkerEvent>> {
        self.render_worker_mut()
            .map_or(Ok(None), AtomicRenderWorkerHandle::try_take_event)
    }

    pub(crate) fn finish_render_work(&self) {
        if let AtomicRenderTransport::Synchronous(domain) = &self.render_transport {
            domain.finish_render_work();
        }
    }

    pub(crate) fn import_scaled_probe_framebuffer(
        &self,
        candidate: &oblivion_one::compositor::DirectScanoutProbeCandidate,
    ) -> io::Result<std::sync::Arc<ImportedDirectFramebuffer>> {
        self.direct
            .framebuffer_cache
            .import_for_probe(&candidate.buffer_identity, &candidate.buffer)
    }

    pub(crate) fn lifecycle_animation_available(&self) -> bool {
        self.lifecycle_animation_available
    }

    pub(crate) fn squash_animation_available(&self) -> bool {
        self.squash_animation_available
    }

    pub(crate) fn reload_trusted_effect_registry(
        &mut self,
        registry: &oblivion_one::effects::TrustedEffectRegistry,
        manifest: oblivion_one::effects::EffectManifest,
    ) -> Result<
        std::sync::Arc<oblivion_one::effects::EffectRegistryGeneration>,
        oblivion_one::effects::RegistryReloadError,
    > {
        match &mut self.render_transport {
            AtomicRenderTransport::Synchronous(domain) => {
                domain.reload_trusted_effect_registry(registry, manifest)
            }
            AtomicRenderTransport::Worker(_) => Err(
                oblivion_one::effects::RegistryReloadError::ReservedSystemEffectName(
                    "trusted effect reload requires an asynchronous render-worker response"
                        .to_string(),
                ),
            ),
        }
    }

    pub(crate) fn publish_material_effect_generation(
        &mut self,
        generation: &oblivion_one::effects::EffectRegistryGeneration,
    ) {
        let _ = self.queue_render_control(AtomicRenderControl::PublishMaterialEffectGeneration(
            generation.clone(),
        ));
    }

    pub(crate) fn publish_effect_registry_generation(
        &mut self,
        generation: oblivion_one::effects::EffectRegistryGeneration,
    ) -> Result<(), oblivion_one::effects::RegistryReloadError> {
        match &mut self.render_transport {
            AtomicRenderTransport::Synchronous(domain) => {
                domain.publish_effect_registry_generation(generation)
            }
            AtomicRenderTransport::Worker(_) => Err(
                oblivion_one::effects::RegistryReloadError::ReservedSystemEffectName(
                    "trusted effect generation publication requires its asynchronous worker response"
                        .to_string(),
                ),
            ),
        }
    }

    pub(crate) fn set_cursor_image(
        &mut self,
        image: std::sync::Arc<oblivion_one::cursor_theme::CompositorCursorImage>,
    ) {
        let _ = self.queue_render_control(AtomicRenderControl::SetCursorImage(image));
    }

    pub(crate) fn invalidate_presented_damage_history(&mut self) {
        let _ = self.queue_render_control(AtomicRenderControl::InvalidatePresentedDamageHistory);
    }
    pub(crate) fn prepare_session_recovery(&self) -> io::Result<AtomicExplicitRecovery> {
        let swapchain = self.swapchain()?;
        let current = swapchain.current();
        Ok(AtomicExplicitRecovery {
            framebuffer: self.framebuffer(current)?,
            current,
            pool_generation: swapchain.pool_generation(),
        })
    }

    pub(crate) fn suspend_for_session(&mut self) -> io::Result<()> {
        self.direct_scanout_suspend()?;
        self.swapchain_mut()?.suspend_abandon_future_frames()?;
        Ok(())
    }

    pub(crate) fn complete_session_recovery(
        &mut self,
        recovery: AtomicExplicitRecovery,
    ) -> io::Result<NativeSessionRecoveryProgress> {
        let swapchain = self.swapchain()?;
        if swapchain.pool_generation() != recovery.pool_generation
            || swapchain.current() != recovery.current
            || self.framebuffer(recovery.current)? != recovery.framebuffer
        {
            return Err(io::Error::other(
                "explicit output recovery token no longer matches the active pool",
            ));
        }
        let suspended_fences_signaled = swapchain.suspended_fences_signaled()?;
        let pending_fence_signaled = swapchain.pending_fence_signaled()?;
        if !suspended_fences_signaled || !pending_fence_signaled {
            return Ok(NativeSessionRecoveryProgress::WaitingForSuspendedFence);
        }
        while let Some(frame) = self.swapchain_mut()?.take_suspended_frame() {
            let _ = self.queue_render_control(AtomicRenderControl::DiscardRendered {
                frame: frame.scene_commit,
            });
            drop(frame.surface_damage);
        }
        self.swapchain_mut()?.recover_suspended_slot(true)?;
        if let Some(frame) = self.swapchain_mut()?.retire_pending_after_recovery() {
            let _ = self.queue_render_control(AtomicRenderControl::DiscardRendered {
                frame: frame.scene_commit,
            });
            drop(frame.surface_damage);
        }
        match self.direct.complete_suspended() {
            DirectReleaseOutcome::Released {
                presented,
                suspended,
            } => {
                drop(presented);
                drop(suspended);
                Ok(NativeSessionRecoveryProgress::Complete)
            }
            DirectReleaseOutcome::Deferred { reason } => Err(io::Error::other(format!(
                "direct ownership release remained deferred after recovery: {reason:?}"
            ))),
            DirectReleaseOutcome::Violation { reason } => Err(io::Error::other(format!(
                "direct ownership release violated after recovery: {reason:?}"
            ))),
        }
    }

    pub(crate) fn release_direct_for_target_destroyed(&mut self) -> io::Result<()> {
        self.release_direct_with_proof(DirectReleaseProof::TargetDestroyed)
    }

    pub(crate) fn release_direct_after_synchronous_modeset(&mut self) -> io::Result<()> {
        self.release_direct_with_proof(DirectReleaseProof::SynchronousModeset)
    }

    pub(crate) fn can_retire_direct_after_synchronous_modeset(&self) -> bool {
        self.direct.ownership.submitted.is_none()
    }

    /// The caller has drained all pageflip and worker ownership and the
    /// synchronous KMS modeset has completed, so the old primary is no longer
    /// scanned out. Moving these owners out and dropping them releases each
    /// client buffer obligation exactly once.
    pub(crate) fn retire_direct_after_synchronous_modeset(&mut self) {
        debug_assert!(self.direct.ownership.submitted.is_none());
        if let DirectReleaseOutcome::Released {
            presented,
            suspended,
        } = self
            .direct
            .ownership
            .request_direct_release(DirectReleaseProof::SynchronousModeset, false)
        {
            drop(presented);
            drop(suspended);
        }
    }

    fn release_direct_with_proof(&mut self, proof: DirectReleaseProof) -> io::Result<()> {
        match self.direct.request_direct_release(proof, false) {
            DirectReleaseOutcome::Released {
                presented,
                suspended,
            } => {
                drop(presented);
                drop(suspended);
                Ok(())
            }
            DirectReleaseOutcome::Deferred { reason } => Err(io::Error::other(format!(
                "direct target-destroyed release remained deferred: {reason:?}"
            ))),
            DirectReleaseOutcome::Violation { reason } => Err(io::Error::other(format!(
                "direct target-destroyed release violated ownership: {reason:?}"
            ))),
        }
    }

    pub(crate) fn retain_direct_for_unproven_teardown(&mut self) {
        let _ = self
            .direct
            .request_direct_release(DirectReleaseProof::Unproven, false);
    }

    pub(crate) fn rebind_session_generation(
        &mut self,
        generation: u64,
        vrr_connector_capable: bool,
        vrr_crtc_property_available: bool,
    ) -> io::Result<()> {
        self.vrr_connector_capable = vrr_connector_capable;
        self.vrr_crtc_property_available = vrr_crtc_property_available;
        let Some(pool) = self.physical_pool.as_ref() else {
            return Ok(());
        };
        validate_split_slot_pairing(
            &pool.identities(),
            &self.render_slot_identities,
            pool.pool_generation(),
        )
        .expect("session recovery starts with exactly paired Atomic pools");
        if let Some(swapchain) = self.swapchain.as_mut() {
            swapchain
                .rebind_pool_generation(generation)
                .expect("recovery retires all non-current explicit output ownership");
        }
        self.physical_pool
            .as_mut()
            .expect("physical pool was checked above")
            .rebind_pool_generation(generation);
        self.queue_render_control(AtomicRenderControl::RebindPoolGeneration(generation))?;
        self.render_slot_identities = self
            .render_slot_identities
            .iter()
            .map(|(id, _)| (*id, generation))
            .collect();
        validate_split_slot_pairing(
            &self
                .physical_pool
                .as_ref()
                .expect("physical pool remains installed during recovery")
                .identities(),
            &self.render_slot_identities,
            generation,
        )
        .expect("session recovery rebinds physical and render pools together");
        self.direct
            .framebuffer_cache
            .clear_for_generation(generation);
        self.direct.drm_generation = generation;
        self.direct.inhibit_until_composited_present = true;
        self.direct.identity_viewport_metadata_logged = false;
        self.direct.last_debug_candidate = None;
        self.dmabuf_scanout_capabilities.output_generation = generation;
        self.async_output_generation = generation;
        self.presentation_validation_accepted.clear();
        self.presentation_validation_rejected.clear();
        self.variable_pageflip_qualification.clear();
        self.queue_render_control(AtomicRenderControl::InvalidatePresentedDamageHistory)?;
        if matches!(&self.render_transport, AtomicRenderTransport::Worker(_)) {
            let (controls, required_epoch) = self.take_render_controls_for_job()?;
            let applied_epoch = self
                .render_worker_mut()
                .ok_or_else(|| io::Error::other("render worker disappeared during session rebind"))?
                .apply_controls_while_quiesced(controls, required_epoch)?;
            if applied_epoch != required_epoch {
                return Err(io::Error::other(
                    "render worker acknowledged a stale session rebind control epoch",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn composited_presentation_validation_key(
        &self,
        output_generation: u64,
        presentation_mode: OutputPresentationMode,
        cursor_visible: bool,
        cursor_state: Option<AtomicCursorVisualState>,
        cursor_transition_pending: bool,
        non_primary_plane_active: bool,
        explicit_sync_ready: bool,
        content_type: oblivion_one::compositor::DrmContentType,
        acquire_strategy: u8,
    ) -> Option<CompositedPresentationValidationKey> {
        (self.async_output_generation == output_generation
            && (!presentation_mode.is_async()
                || (self.async_page_flip_capable && self.async_format_capable)))
            .then_some(CompositedPresentationValidationKey::new(
                self.direct.output_id,
                output_generation,
                self.async_crtc_id,
                self.async_primary_plane_id,
                self.format_modifier,
                presentation_mode,
                acquire_strategy,
                cursor_visible,
                cursor_state,
                cursor_transition_pending,
                non_primary_plane_active,
                explicit_sync_ready,
                content_type,
            ))
    }

    fn resolve_connector_content_type(&self, requested: DrmContentType) -> DrmContentType {
        if self.connector_content_types.contains(&requested) {
            requested
        } else {
            DrmContentType::Graphics
        }
    }

    fn effective_presentation_for_scene(
        &self,
        server: &OwnCompositorServer,
        output_generation: u64,
        pacing_mode: NativeOutputPacingMode,
        async_policy_inputs: AtomicAsyncPolicyInputs,
        cursor_visible: bool,
        vrr_policy: VrrPolicy,
    ) -> (DrmContentType, EffectivePresentation) {
        let metadata = server
            .fullscreen_tree_presentation_metadata()
            .unwrap_or_default();
        let metrics = server.fullscreen_render_plan_metrics();
        let content_type = self.resolve_connector_content_type(metadata.content_type.drm_value());
        let effective = EffectivePresentation::decide(
            TearingPolicy::from_environment(std::env::var("OBLIVION_ONE_TEARING").ok().as_deref()),
            vrr_policy,
            metadata,
            AsyncEligibility {
                solitary_fullscreen: metrics.solitary_tree_active,
                async_hint: metadata.hint.is_async(),
                backend_capable: self.async_page_flip_capable,
                async_format_supported: self.async_format_capable,
                output_generation_qualified: self.async_output_generation == output_generation,
                explicit_sync_ready: self.swapchain.is_some(),
                commit_timing_safe: pacing_mode == NativeOutputPacingMode::ReactiveDouble,
                kms_lane_free: async_policy_inputs.kms_lane_free,
                async_test_only_accepted: true,
                modeset_required: content_type
                    != async_policy_inputs
                        .confirmed_presentation_state
                        .content_type,
                cursor_visible,
                cursor_transition_pending: async_policy_inputs.cursor_transition_pending,
                ..AsyncEligibility::default()
            },
            VrrEligibility {
                auto_candidate: metrics.solitary_tree_active,
                backend_capable: true,
                connector_capable: self.vrr_connector_capable,
                crtc_property_available: self.vrr_crtc_property_available,
                output_generation_qualified: self.async_output_generation == output_generation,
                exact_kms_qualified: true,
                transition_supported: true,
            },
        );
        (content_type, effective)
    }

    pub(crate) fn prepare_composited_presentation_state(
        &self,
        server: &OwnCompositorServer,
        output_generation: u64,
        pacing_mode: NativeOutputPacingMode,
        async_policy_inputs: AtomicAsyncPolicyInputs,
        cursor: Option<&CursorPlaneAssignment>,
        vrr_policy: VrrPolicy,
    ) -> PreparedCompositedPresentationState {
        let cursor_visible = matches!(
            cursor,
            Some(CursorPlaneAssignment::Atomic {
                state: Some(state),
                ..
            }) if state.visible
        );
        let cursor_state = match cursor {
            Some(CursorPlaneAssignment::Atomic { state, .. }) => state.clone(),
            Some(CursorPlaneAssignment::Unchanged | CursorPlaneAssignment::Disabled) | None => None,
        };
        let (content_type, effective_presentation) = self.effective_presentation_for_scene(
            server,
            output_generation,
            pacing_mode,
            async_policy_inputs,
            cursor_visible,
            vrr_policy,
        );
        let mut mode = effective_presentation.mode;
        let mut policy_pacing_mode = pacing_mode;
        if mode.uses_vrr() {
            policy_pacing_mode = NativeOutputPacingMode::ReactiveDouble;
        }
        let acquire_strategy = match policy_pacing_mode {
            NativeOutputPacingMode::ReactiveDouble => 0,
            NativeOutputPacingMode::PredictiveTriple => 1,
        };
        let mut vrr_blocker = effective_presentation.vrr_blocker;
        let mut async_blocker = effective_presentation.async_blocker;
        let mut combination_blocker = None;
        if self
            .variable_pageflip_qualification
            .rejects_mode(mode, output_generation)
        {
            mode = OutputPresentationMode::Vsync;
            vrr_blocker = Some(VrrBlocker::ExactKmsQualificationRejected);
        }
        if mode != OutputPresentationMode::Vsync
            && let Some(mut rejected_key) = self.composited_presentation_validation_key(
                output_generation,
                mode,
                cursor_visible,
                cursor_state.clone(),
                async_policy_inputs.cursor_transition_pending,
                false,
                self.swapchain.is_some(),
                content_type,
                acquire_strategy,
            )
            && self.presentation_validation_is_rejected(rejected_key)
        {
            match mode {
                OutputPresentationMode::AdaptiveAsync => {
                    rejected_key.presentation_mode = OutputPresentationMode::AdaptiveSync;
                    let adaptive_sync_key = rejected_key;
                    rejected_key.presentation_mode = OutputPresentationMode::Async;
                    let async_key = rejected_key;
                    (mode, vrr_blocker, async_blocker, combination_blocker) =
                        atomic_egl_gbm_transactions::adaptive_async_fallback_diagnostics(
                            self.presentation_validation_is_accepted(adaptive_sync_key),
                            self.presentation_validation_is_accepted(async_key),
                        );
                }
                OutputPresentationMode::AdaptiveSync => {
                    mode = OutputPresentationMode::Vsync;
                    vrr_blocker = Some(VrrBlocker::ExactKmsQualificationRejected);
                }
                OutputPresentationMode::Async => {
                    mode = OutputPresentationMode::Vsync;
                    async_blocker = Some(AsyncBlocker::AsyncTestOnlyRejected);
                }
                OutputPresentationMode::Vsync => {}
            }
        }
        let validation_key = (mode != OutputPresentationMode::Vsync)
            .then(|| {
                self.composited_presentation_validation_key(
                    output_generation,
                    mode,
                    cursor_visible,
                    cursor_state,
                    async_policy_inputs.cursor_transition_pending,
                    false,
                    self.swapchain.is_some(),
                    content_type,
                    acquire_strategy,
                )
            })
            .flatten();
        let validation_accepted =
            validation_key.is_some_and(|key| self.presentation_validation_is_accepted(key));
        let desired_state = OutputPresentationStateKey {
            mode,
            content_type,
            output_generation,
        };
        let confirmed_state_matches =
            async_policy_inputs.confirmed_presentation_state == desired_state;
        let presentation_state_transition_required = !kms_presentation_state_is_represented(
            desired_state,
            async_policy_inputs.confirmed_presentation_state,
            async_policy_inputs.pending_presentation_state,
        );
        NativePerfLogger::from_env().log("native.output_presentation_policy", || {
            vec![
                NativePerfField::str("configured_policy", vrr_policy.as_str()),
                NativePerfField::bool("drm_connector_capable", self.vrr_connector_capable),
                NativePerfField::bool(
                    "crtc_vrr_property_available",
                    self.vrr_crtc_property_available,
                ),
                NativePerfField::str("effective_mode", mode.as_str()),
                NativePerfField::str(
                    "vrr_blocker",
                    vrr_blocker.map_or("none", VrrBlocker::as_str),
                ),
                NativePerfField::str(
                    "async_blocker",
                    async_blocker.map_or("none", AsyncBlocker::as_str),
                ),
                NativePerfField::str("combination_blocker", combination_blocker.unwrap_or("none")),
                NativePerfField::u64("output_generation", output_generation),
            ]
        });
        PreparedCompositedPresentationState {
            mode,
            content_type,
            validation_key,
            validation_accepted,
            confirmed_state_matches,
            presentation_state_transition_required,
            vrr_blocker,
            async_blocker,
            combination_blocker,
            acquire_strategy,
        }
    }

    pub(crate) fn presentation_validation_is_accepted(
        &self,
        key: CompositedPresentationValidationKey,
    ) -> bool {
        self.presentation_validation_accepted.contains(&key)
    }

    pub(crate) fn presentation_validation_is_rejected(
        &self,
        key: CompositedPresentationValidationKey,
    ) -> bool {
        self.presentation_validation_rejected.contains(&key)
    }

    pub(crate) fn note_composited_presentation_validation(
        &mut self,
        key: CompositedPresentationValidationKey,
        accepted: bool,
    ) {
        if accepted {
            self.presentation_validation_rejected.remove(&key);
            self.presentation_validation_accepted.insert(key);
        } else {
            self.presentation_validation_accepted.remove(&key);
            self.presentation_validation_rejected.insert(key);
        }
    }

    pub(crate) fn reject_variable_pageflip_for_generation(&mut self, output_generation: u64) {
        if self.async_output_generation == output_generation {
            self.variable_pageflip_qualification
                .reject_generation(output_generation);
        }
    }

    pub(crate) fn invalidate_direct_validation_cache(&mut self) {
        self.direct.invalidate_direct_validation_cache();
    }

    pub(crate) fn start_worker_output_configuration_candidate(
        &mut self,
        kms: &fs::File,
        discovery: AtomicDiscovery,
        width: u32,
        height: u32,
        output_generation: u64,
        pool_generation: u64,
        job_id: RenderWorkerJobId,
        frame_id: u64,
        frozen_frame: FrozenNativeRenderFrame,
        material_generation: oblivion_one::effects::EffectRegistryGeneration,
        effect_generation: oblivion_one::effects::EffectRegistryGeneration,
        cursor_image: std::sync::Arc<oblivion_one::cursor_theme::CompositorCursorImage>,
    ) -> io::Result<AtomicOutputConfigurationCandidate> {
        if !matches!(&self.render_transport, AtomicRenderTransport::Worker(_)) {
            return Err(io::Error::other(
                "asynchronous output candidate requires the active Atomic render worker",
            ));
        }
        if self.render_worker_busy() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "render worker is busy before output candidate preparation",
            ));
        }

        let physical_fd =
            duplicate_fd_cloexec(kms.as_raw_fd()).map_err(io::Error::from_raw_os_error)?;
        let drm_rdev = kms.metadata()?.rdev();
        let physical_device = gbm::Device::new(physical_fd)?;
        let mut probe = DeviceAllocationProbe {
            device: &physical_device,
            width,
            height,
        };
        let format_modifier = select_output_format_modifier_for_presentation(
            oblivion_one::compositor::OutputPresentationMode::Vsync,
            &discovery.plane_scanout_formats,
            Some(&discovery.plane_async_scanout_formats),
            &self.renderable_dmabuf_formats,
            &mut probe,
        )?;
        let mut physical_pool = create_physical_output_pool(
            &physical_device,
            kms.as_fd(),
            width,
            height,
            format_modifier,
            pool_generation,
        )?;
        let slot = OutputSlotId::new(0).expect("slot zero is valid");
        let descriptors = match physical_pool.export_render_slot_descriptors() {
            Ok(descriptors) => descriptors,
            Err(error) => {
                physical_pool.teardown(physical_device.as_fd(), true);
                return Err(error);
            }
        };
        let render_device_fd = match duplicate_fd_cloexec(kms.as_raw_fd()) {
            Ok(fd) => fd,
            Err(error) => {
                physical_pool.teardown(physical_device.as_fd(), true);
                return Err(io::Error::from_raw_os_error(error));
            }
        };
        let target = match physical_pool.render_target_request(slot, 0) {
            Ok(target) => target,
            Err(error) => {
                physical_pool.teardown(physical_device.as_fd(), true);
                return Err(error);
            }
        };
        let identity =
            AtomicRenderCandidateIdentity::new(job_id, output_generation, pool_generation, slot);
        let (controls_before, required_control_epoch) = match self.take_render_controls_for_job() {
            Ok(controls) => controls,
            Err(error) => {
                physical_pool.teardown(physical_device.as_fd(), true);
                return Err(error);
            }
        };
        let job = AtomicRenderCandidateJob {
            identity,
            render_device_fd,
            width,
            height,
            format_modifier,
            physical_slot_identities: physical_pool.identities(),
            descriptors,
            target,
            frame_id,
            frozen_frame,
            material_generation,
            effect_generation,
            cursor_image,
            required_control_epoch,
            controls_before,
        };
        let Some(worker) = self.render_worker_mut() else {
            physical_pool.teardown(physical_device.as_fd(), true);
            return Err(io::Error::other(
                "render worker disappeared during candidate setup",
            ));
        };
        if let Err(job) = worker.try_prepare_candidate(job) {
            self.pending_render_controls = job.controls_before;
            physical_pool.teardown(physical_device.as_fd(), true);
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "render worker rejected output candidate preparation",
            ));
        }

        Ok(AtomicOutputConfigurationCandidate {
            identity,
            physical_device: Some(physical_device),
            physical_pool: Some(physical_pool),
            discovery: Some(discovery),
            render_domain_may_own: true,
            drm_rdev,
            format_modifier,
            width,
            height,
        })
    }

    pub(crate) fn try_promote_worker_output_candidate(
        &mut self,
        identity: AtomicRenderCandidateIdentity,
    ) -> io::Result<bool> {
        if self.render_worker_busy() {
            return Ok(false);
        }
        let (controls_before, required_control_epoch) = self.take_render_controls_for_job()?;
        let Some(worker) = self.render_worker_mut() else {
            self.pending_render_controls = controls_before;
            return Err(io::Error::other(
                "render worker disappeared before candidate promotion",
            ));
        };
        match worker.try_promote_candidate(identity, controls_before, required_control_epoch) {
            Ok(()) => Ok(true),
            Err((error, controls_before)) => {
                self.pending_render_controls = controls_before;
                if error.kind() == io::ErrorKind::WouldBlock {
                    Ok(false)
                } else {
                    Err(error)
                }
            }
        }
    }

    pub(crate) fn try_discard_worker_output_candidate(
        &mut self,
        identity: AtomicRenderCandidateIdentity,
    ) -> io::Result<bool> {
        if self.render_worker_busy() {
            return Ok(false);
        }
        let Some(worker) = self.render_worker_mut() else {
            return Err(io::Error::other(
                "render worker disappeared before candidate discard",
            ));
        };
        match worker.try_discard_candidate(identity) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn adopt_worker_output_candidate(
        &mut self,
        mut candidate: AtomicOutputConfigurationCandidate,
        frame: Box<AtomicRenderedFrameParts>,
        render_info: AtomicRenderDomainInfo,
    ) -> io::Result<()> {
        if !matches!(&self.render_transport, AtomicRenderTransport::Worker(_)) {
            return Err(io::Error::other(
                "cannot adopt a worker output candidate without worker ownership",
            ));
        }
        if candidate.identity.pool_generation != candidate.pool_generation() {
            return Err(io::Error::other(
                "candidate physical pool generation mismatch",
            ));
        }
        let (physical_device, physical_pool, discovery, drm_rdev, format_modifier, width, height) =
            candidate.take_parts()?;
        let physical_identities = physical_pool.identities();
        if physical_pool.pool_generation() != candidate.identity.pool_generation
            || !physical_identities
                .contains(&(candidate.identity.slot, candidate.identity.pool_generation))
            || physical_identities
                .iter()
                .any(|(_, generation)| *generation != candidate.identity.pool_generation)
        {
            std::mem::forget(physical_pool);
            std::mem::forget(physical_device);
            return Err(io::Error::other(
                "candidate physical pool does not match its render identity",
            ));
        }
        let scanout_capabilities = discover_direct_scanout_capabilities(
            &discovery.plane_scanout_formats,
            &render_info.dmabuf_feedback,
        );
        let scanout_capabilities = DirectScanoutFeedbackCapabilities::new(
            drm_rdev,
            candidate.identity.pool_generation,
            discovery.pipeline.plane.get(),
            scanout_capabilities,
        );
        let kms_presentable_client_formats = discover_kms_presentable_client_formats(
            &discovery.plane_scanout_formats,
            render_info.dmabuf_feedback.formats(),
        );
        let kms_preferred_selection = resolve_kms_preferred_dmabuf_policy(
            KmsPreferredDmabufPolicy::from_env(),
            &render_info.egl_vendor,
            render_info.dmabuf_feedback.formats(),
            &kms_presentable_client_formats,
        );
        self.dmabuf_kms_preferred_state = kms_preferred_selection.summary();
        self.dmabuf_feedback = EglGlesDmabufFeedback::with_scanout_tranche(
            scanout_capabilities.formats.iter().map(|format| {
                EglGlesDmabufFormat::new(
                    DrmFormat::from_fourcc(format.format),
                    DrmModifier(format.modifier),
                )
            }),
            kms_preferred_selection.advertised_renderer_formats,
        );
        self.dmabuf_scanout_capabilities = scanout_capabilities;
        self.dmabuf_main_device = render_info.dmabuf_main_device;
        self.dmabuf_main_device_path = render_info.dmabuf_main_device_path;
        self.dmabuf_egl_vendor = render_info.egl_vendor;
        self.lifecycle_animation_available = render_info.lifecycle_animation_available;
        self.squash_animation_available = render_info.squash_animation_available;
        self.renderable_dmabuf_formats = render_info.dmabuf_feedback.formats().to_vec();
        self.async_page_flip_capable = discovery.optional.async_page_flip;
        self.async_format_capable = discovery
            .plane_async_scanout_formats
            .contains(&format_modifier);
        self.vrr_connector_capable = discovery.pipeline.connector_props.vrr_capable.is_some()
            && discovery
                .pipeline
                .connector_props
                .vrr_capable_value
                .is_some_and(|value| value != 0);
        self.vrr_crtc_property_available = discovery.pipeline.crtc_props.vrr_enabled.is_some();
        self.connector_content_types = [
            DrmContentType::Graphics,
            DrmContentType::Photo,
            DrmContentType::Cinema,
            DrmContentType::Game,
        ]
        .into_iter()
        .filter(|content_type| {
            discovery
                .pipeline
                .connector_props
                .content_type_value(content_type.as_str())
                .is_some()
        })
        .collect();
        self.async_crtc_id = discovery.pipeline.crtc.get();
        self.async_primary_plane_id = discovery.pipeline.plane.get();
        self.async_output_generation = candidate.identity.pool_generation;

        let output_id = self.direct.output_id;
        let old_pool = self.physical_pool.replace(physical_pool);
        let old_device = self._physical_device.replace(physical_device);
        self.direct = DirectScanoutControl::new(
            self._physical_device
                .as_ref()
                .expect("candidate physical device installed")
                .as_fd(),
            candidate.identity.pool_generation,
            output_id,
        );
        self.width = width;
        self.height = height;
        self.format_modifier = format_modifier;
        self.render_slot_identities = self
            .physical_pool
            .as_ref()
            .expect("candidate physical pool installed")
            .identities();
        self.swapchain = None;
        if let (Some(pool), Some(device)) = (old_pool, old_device.as_ref()) {
            pool.teardown(device.as_fd(), self.drm_cleanup_armed);
        }
        drop(old_device);
        self.promote_initial_presented(frame.slot, frame.scene_commit)
    }

    pub(crate) fn create_unattached_pool(
        kms: &fs::File,
        discovery: &AtomicDiscovery,
        width: u32,
        height: u32,
        pool_generation: u64,
        output_id: OutputId,
        initial_generation: std::sync::Arc<oblivion_one::effects::EffectRegistryGeneration>,
        cursor_image: std::sync::Arc<oblivion_one::cursor_theme::CompositorCursorImage>,
    ) -> io::Result<Self> {
        let worker_policy = AtomicRenderWorkerPolicy::parse(
            std::env::var("OBLIVION_ONE_RENDER_WORKER").ok().as_deref(),
        )
        .map_err(io::Error::other)?;
        Self::create_unattached_pool_with_policy(
            kms,
            discovery,
            width,
            height,
            pool_generation,
            output_id,
            worker_policy,
            initial_generation,
            cursor_image,
        )
    }

    /// Output reconfiguration on an already synchronous transport must stay
    /// synchronous; it must not create a render worker from the event loop.
    pub(crate) fn create_synchronous_unattached_pool(
        kms: &fs::File,
        discovery: &AtomicDiscovery,
        width: u32,
        height: u32,
        pool_generation: u64,
        output_id: OutputId,
        initial_generation: std::sync::Arc<oblivion_one::effects::EffectRegistryGeneration>,
        cursor_image: std::sync::Arc<oblivion_one::cursor_theme::CompositorCursorImage>,
    ) -> io::Result<Self> {
        Self::create_unattached_pool_with_policy(
            kms,
            discovery,
            width,
            height,
            pool_generation,
            output_id,
            AtomicRenderWorkerPolicy::Off,
            initial_generation,
            cursor_image,
        )
    }

    fn create_unattached_pool_with_policy(
        kms: &fs::File,
        discovery: &AtomicDiscovery,
        width: u32,
        height: u32,
        pool_generation: u64,
        output_id: OutputId,
        worker_policy: AtomicRenderWorkerPolicy,
        initial_generation: std::sync::Arc<oblivion_one::effects::EffectRegistryGeneration>,
        cursor_image: std::sync::Arc<oblivion_one::cursor_theme::CompositorCursorImage>,
    ) -> io::Result<Self> {
        let physical_fd =
            duplicate_fd_cloexec(kms.as_raw_fd()).map_err(io::Error::from_raw_os_error)?;
        let physical_device = gbm::Device::new(physical_fd)?;
        let (render_transport, format_modifier, render_info, physical_pool, worker_startup) =
            match worker_policy {
                AtomicRenderWorkerPolicy::Off => create_synchronous_render_setup(
                    kms,
                    discovery,
                    &physical_device,
                    width,
                    height,
                    pool_generation,
                )
                .map(|setup| {
                    (
                        setup.0,
                        setup.1,
                        setup.2,
                        setup.3,
                        AtomicRenderWorkerStartup::IntentionallySynchronous,
                    )
                })?,
                AtomicRenderWorkerPolicy::Auto => {
                    match create_worker_render_setup(
                        kms,
                        discovery,
                        &physical_device,
                        width,
                        height,
                        pool_generation,
                        std::sync::Arc::clone(&initial_generation),
                        std::sync::Arc::clone(&cursor_image),
                    ) {
                        Ok(setup) => (
                            setup.0,
                            setup.1,
                            setup.2,
                            setup.3,
                            AtomicRenderWorkerStartup::WorkerStarted,
                        ),
                        Err(worker_error) => {
                            eprintln!(
                                "native render worker: requested=auto effective=synchronous startup=automatic_startup_degraded reason=worker_bootstrap_failed error={worker_error}"
                            );
                            create_synchronous_render_setup(
                                kms,
                                discovery,
                                &physical_device,
                                width,
                                height,
                                pool_generation,
                            )
                            .map(|setup| {
                                (
                                    setup.0,
                                    setup.1,
                                    setup.2,
                                    setup.3,
                                    AtomicRenderWorkerStartup::AutomaticStartupDegraded,
                                )
                            })?
                        }
                    }
                }
                AtomicRenderWorkerPolicy::Force => {
                    let setup = create_worker_render_setup(
                        kms,
                        discovery,
                        &physical_device,
                        width,
                        height,
                        pool_generation,
                        std::sync::Arc::clone(&initial_generation),
                        std::sync::Arc::clone(&cursor_image),
                    )?;
                    (
                        setup.0,
                        setup.1,
                        setup.2,
                        setup.3,
                        AtomicRenderWorkerStartup::WorkerStarted,
                    )
                }
            };
        let transport_kind = match &render_transport {
            AtomicRenderTransport::Synchronous(_) => AtomicRenderTransportKind::Synchronous,
            AtomicRenderTransport::Worker(_) => AtomicRenderTransportKind::Worker,
        };
        let render_slot_identities = physical_pool.identities();
        println!(
            "native Atomic render transport: requested={} effective={} startup={} two_stage_bootstrap={}",
            worker_policy.as_str(),
            transport_kind.as_str(),
            worker_startup.as_str(),
            matches!(transport_kind, AtomicRenderTransportKind::Worker),
        );

        let renderable_dmabuf_formats = render_info.dmabuf_feedback.formats().to_vec();
        let scanout_capabilities = discover_direct_scanout_capabilities(
            &discovery.plane_scanout_formats,
            &render_info.dmabuf_feedback,
        );
        let scanout_capabilities = DirectScanoutFeedbackCapabilities::new(
            kms.metadata()?.rdev(),
            pool_generation,
            discovery.pipeline.plane.get(),
            scanout_capabilities,
        );
        let kms_presentable_client_formats = discover_kms_presentable_client_formats(
            &discovery.plane_scanout_formats,
            render_info.dmabuf_feedback.formats(),
        );
        let kms_preferred_selection = resolve_kms_preferred_dmabuf_policy(
            KmsPreferredDmabufPolicy::from_env(),
            &render_info.egl_vendor,
            render_info.dmabuf_feedback.formats(),
            &kms_presentable_client_formats,
        );
        let dmabuf_kms_preferred_state = kms_preferred_selection.summary();
        let dmabuf_feedback = EglGlesDmabufFeedback::with_scanout_tranche(
            scanout_capabilities.formats.iter().map(|format| {
                EglGlesDmabufFormat::new(
                    DrmFormat::from_fourcc(format.format),
                    DrmModifier(format.modifier),
                )
            }),
            kms_preferred_selection.advertised_renderer_formats,
        );
        Ok(Self {
            _physical_device: Some(physical_device),
            physical_pool: Some(physical_pool),
            render_transport,
            pending_render_controls: Vec::new(),
            render_control_epoch: 0,
            render_control_overflowed: false,
            render_slot_identities,
            lifecycle_animation_available: render_info.lifecycle_animation_available,
            squash_animation_available: render_info.squash_animation_available,
            swapchain: None,
            direct: DirectScanoutControl::new(kms.as_fd(), pool_generation, output_id),
            width,
            height,
            dmabuf_feedback,
            dmabuf_main_device: render_info.dmabuf_main_device,
            dmabuf_main_device_path: render_info.dmabuf_main_device_path,
            dmabuf_egl_vendor: render_info.egl_vendor,
            dmabuf_scanout_capabilities: scanout_capabilities,
            dmabuf_kms_preferred_state,
            format_modifier,
            drm_cleanup_armed: true,
            renderable_dmabuf_formats,
            deadline_hints_enabled: true,
            counters: ExplicitOutputCounters::default(),
            async_page_flip_capable: discovery.optional.async_page_flip,
            async_format_capable: discovery
                .plane_async_scanout_formats
                .contains(&format_modifier),
            vrr_connector_capable: discovery.pipeline.connector_props.vrr_capable.is_some()
                && discovery
                    .pipeline
                    .connector_props
                    .vrr_capable_value
                    .is_some_and(|value| value != 0),
            vrr_crtc_property_available: discovery.pipeline.crtc_props.vrr_enabled.is_some(),
            connector_content_types: [
                DrmContentType::Graphics,
                DrmContentType::Photo,
                DrmContentType::Cinema,
                DrmContentType::Game,
            ]
            .into_iter()
            .filter(|content_type| {
                discovery
                    .pipeline
                    .connector_props
                    .content_type_value(content_type.as_str())
                    .is_some()
            })
            .collect(),
            async_crtc_id: discovery.pipeline.crtc.get(),
            async_primary_plane_id: discovery.pipeline.plane.get(),
            async_output_generation: pool_generation,
            presentation_validation_accepted: HashSet::new(),
            presentation_validation_rejected: HashSet::new(),
            variable_pageflip_qualification: VariablePageflipQualification::default(),
        })
    }

    pub(crate) fn create_render_fence(&self) -> io::Result<NativeRenderFence> {
        match &self.render_transport {
            AtomicRenderTransport::Synchronous(domain) => domain.create_render_fence(),
            AtomicRenderTransport::Worker(_) => Err(io::Error::other(
                "render fence creation must run as a render-worker command",
            )),
        }
    }

    pub(crate) fn duplicate_ready_render_completion_fd(&self) -> io::Result<std::os::fd::OwnedFd> {
        self.swapchain()?.duplicate_ready_render_completion_fd()
    }

    pub(crate) fn initial_slot(&self) -> OutputSlotId {
        OutputSlotId::new(0).expect("slot zero is valid")
    }

    pub(crate) const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub(crate) fn framebuffer(&self, slot: OutputSlotId) -> io::Result<FramebufferId> {
        Ok(self.physical_slot(slot)?.framebuffer)
    }

    /// Abandon an unpresented render after the caller has proven completion of
    /// all GLES work for this context. The slot's recorded presentation age is
    /// invalidated before its rendering ownership is released.
    fn complete_unpresented_render(&mut self, slot: OutputSlotId) -> io::Result<()> {
        if self.swapchain()?.rendering_slot() != Some(slot) {
            return Err(io::Error::other(
                "completed unpresented output slot does not match active rendering ownership",
            ));
        }
        self.physical_slot_mut(slot)?
            .invalidate_after_unpresented_render();
        self.swapchain_mut()?.complete_unpresented_render(slot)
    }

    pub(crate) fn plane_count(&self) -> io::Result<u32> {
        self.physical_pool
            .as_ref()
            .ok_or_else(|| io::Error::other("explicit output pool is unavailable"))?
            .plane_count()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_to_slot(
        &mut self,
        target: RenderTargetRequest,
        frame_id: u64,
        frozen_frame: &mut FrozenNativeRenderFrame,
        gpu_sampling_started: &mut bool,
        phase_durations: Option<&mut AtomicRenderPhaseDurations>,
    ) -> io::Result<AtomicSlotRenderOutcome> {
        match &mut self.render_transport {
            AtomicRenderTransport::Synchronous(domain) => domain.render_to_slot(
                target,
                frame_id,
                frozen_frame,
                gpu_sampling_started,
                phase_durations,
            ),
            AtomicRenderTransport::Worker(_) => Err(io::Error::other(
                "worker transport requires reactor-owned render dispatch",
            )),
        }
    }

    pub(crate) fn render_initial_frame(
        &mut self,
        target: RenderTargetRequest,
        frame_id: u64,
        frozen_frame: FrozenNativeRenderFrame,
    ) -> io::Result<(AtomicSlotRenderOutcome, FrozenNativeRenderFrame)> {
        if matches!(&self.render_transport, AtomicRenderTransport::Worker(_)) {
            let (controls_before, required_control_epoch) = self.take_render_controls_for_job()?;
            return self
                .render_worker_mut()
                .ok_or_else(|| io::Error::other("render worker disappeared during bootstrap"))?
                .render_initial_frame(AtomicRenderWorkerInitialFrame {
                    target,
                    frame_id,
                    frozen_frame,
                    required_control_epoch,
                    controls_before,
                });
        }
        match &mut self.render_transport {
            AtomicRenderTransport::Synchronous(domain) => {
                let mut frozen_frame = frozen_frame;
                let mut gpu_sampling_started = false;
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
                result.map(|outcome| (outcome, frozen_frame))
            }
            AtomicRenderTransport::Worker(_) => unreachable!("worker handled before sync borrow"),
        }
    }

    /// Capture the published scene without acquiring a physical output slot.
    /// The normal Atomic context is made current, but no KMS or swapchain
    /// state participates in this operation.
    pub(crate) fn capture_scene(
        &mut self,
        resolved_scene: ResolvedNativeFrameScene<'static>,
        server: &OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
    ) -> io::Result<Vec<u8>> {
        let mut frozen_frame = FrozenNativeRenderFrame::freeze(
            self.width,
            self.height,
            resolved_scene,
            server,
            input_state,
            cursor_mode,
            Some(OutputDamage::Full),
        );
        match &mut self.render_transport {
            AtomicRenderTransport::Synchronous(domain) => domain.capture_scene(&mut frozen_frame),
            AtomicRenderTransport::Worker(_) => Err(io::Error::other(
                "screen capture must be admitted as a render-worker operation",
            )),
        }
    }

    pub(crate) fn freeze_capture_frame(
        &self,
        resolved_scene: ResolvedNativeFrameScene<'static>,
        server: &OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
    ) -> FrozenNativeRenderFrame {
        FrozenNativeRenderFrame::freeze(
            self.width,
            self.height,
            resolved_scene,
            server,
            input_state,
            cursor_mode,
            Some(OutputDamage::Full),
        )
    }

    pub(crate) fn dispatch_capture(
        &mut self,
        job_id: RenderWorkerJobId,
        frozen_frame: FrozenNativeRenderFrame,
    ) -> io::Result<u64> {
        let (controls_before, required_control_epoch) = self.take_render_controls_for_job()?;
        let capture = AtomicRenderWorkerCapture {
            job_id,
            frozen_frame,
            required_control_epoch,
            controls_before,
        };
        match self
            .render_worker_mut()
            .ok_or_else(|| io::Error::other("capture dispatch requires render-worker mode"))?
            .try_dispatch_capture(capture)
        {
            Ok(()) => Ok(required_control_epoch),
            Err(capture) => {
                self.pending_render_controls = capture.controls_before;
                Err(io::Error::other(
                    "render worker rejected a serialized capture command",
                ))
            }
        }
    }

    pub(crate) fn dispatch_effect_reload(
        &mut self,
        job_id: RenderWorkerJobId,
        previous_generation: std::sync::Arc<oblivion_one::effects::EffectRegistryGeneration>,
    ) -> io::Result<Option<u64>> {
        if !self.render_worker().is_some_and(|worker| !worker.busy()) {
            return Ok(None);
        }
        let (controls_before, required_control_epoch) = self.take_render_controls_for_job()?;
        let reload = AtomicRenderWorkerEffectReload {
            job_id,
            previous_generation,
            manifest: None,
            required_control_epoch,
            controls_before,
        };
        match self
            .render_worker_mut()
            .ok_or_else(|| io::Error::other("effect reload requires render-worker mode"))?
            .try_dispatch_effect_reload(reload)
        {
            Ok(()) => Ok(Some(required_control_epoch)),
            Err(reload) => {
                self.pending_render_controls = reload.controls_before;
                Ok(None)
            }
        }
    }

    pub(crate) fn reload_effect_registry_before_event_loop(
        &mut self,
        job_id: RenderWorkerJobId,
        previous_generation: std::sync::Arc<oblivion_one::effects::EffectRegistryGeneration>,
        manifest_path: String,
        manifest: oblivion_one::effects::EffectManifest,
    ) -> Result<Option<super::render_worker::PreparedWorkerEffectReload>, String> {
        if !self.render_worker().is_some_and(|worker| !worker.busy()) {
            return Err(String::from(
                "startup effect reload requires an idle Atomic render worker",
            ));
        }
        let (controls_before, required_control_epoch) = self
            .take_render_controls_for_job()
            .map_err(|error| error.to_string())?;
        let reload = AtomicRenderWorkerEffectReload {
            job_id,
            previous_generation,
            manifest: Some((manifest_path, manifest)),
            required_control_epoch,
            controls_before,
        };
        let Some(worker) = self.render_worker_mut() else {
            self.pending_render_controls = reload.controls_before;
            return Err(String::from(
                "startup effect reload requires the Atomic render worker",
            ));
        };
        worker.reload_effects_before_event_loop(reload)
    }

    pub(crate) fn dispatch_deferred_release_fence(
        &mut self,
        job_id: RenderWorkerJobId,
    ) -> io::Result<Option<u64>> {
        if !self.render_worker().is_some_and(|worker| !worker.busy()) {
            return Ok(None);
        }
        let (controls_before, required_control_epoch) = self.take_render_controls_for_job()?;
        let request = AtomicRenderWorkerReleaseFence {
            job_id,
            required_control_epoch,
            controls_before,
        };
        match self
            .render_worker_mut()
            .ok_or_else(|| io::Error::other("release fence requires render-worker mode"))?
            .try_dispatch_release_fence(request)
        {
            Ok(()) => Ok(Some(required_control_epoch)),
            Err(request) => {
                self.pending_render_controls = request.controls_before;
                Ok(None)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_frame(
        &mut self,
        server: &mut OwnCompositorServer,
        output_transactions: &mut OutputTransactionLedger,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
        damage: &NativeOutputDamage,
        resolved_scene: ResolvedNativeFrameScene<'static>,
        render_generation: u64,
        output_generation: u64,
        reservation: PreparedCompositedReservation,
        prepared_presentation: PreparedCompositedPresentationState,
        pacing_mode: NativeOutputPacingMode,
        render_ahead: bool,
        future_primary_limit: u8,
        o1_admission: Option<O1AdmissionObservation>,
        cursor: Option<CursorPlaneAssignment>,
        equivalent_direct_key: Option<DirectScanoutCandidateKey>,
        frozen_cursor_plan: crate::native_output::presentation::plane::FrozenPrimaryCursorPlan,
        frozen_cursor_plane_owner: Option<FrozenCursorPlaneOwner>,
        frozen_cursor_trace_reveal: Option<
            crate::native_output::presentation::plane::CursorRevealTraceSnapshot,
        >,
        dmabuf_gpu_release_safety: DmabufGpuReleaseSafety,
        dmabuf_gpu_release_lease_id: Option<oblivion_one::compositor::DmabufGpuReleaseLeaseId>,
        mut phase_durations: Option<&mut AtomicRenderPhaseDurations>,
    ) -> io::Result<AtomicFrameRenderOutcome> {
        let presentation_mode = prepared_presentation.mode;
        let content_type = prepared_presentation.content_type;
        let presentation_validation_key = prepared_presentation.validation_key;
        let presentation_state_transition_required =
            prepared_presentation.presentation_state_transition_required;
        let mut pacing_mode = pacing_mode;
        let mut render_ahead = render_ahead;
        if presentation_mode.uses_vrr() {
            render_ahead = false;
            pacing_mode = NativeOutputPacingMode::ReactiveDouble;
        }
        let render_damage = damage_for_physical_presentation_state(
            damage,
            presentation_state_transition_required,
            self.width,
            self.height,
        );
        let pacing_mode = if presentation_mode.is_async() {
            NativeOutputPacingMode::ReactiveDouble
        } else {
            pacing_mode
        };
        let slot_acquisition_started = phase_durations.as_ref().map(|_| Instant::now());
        let slot_result = {
            let swapchain = self.swapchain_mut()?;
            swapchain.acquire_render_slot_for_reservation(
                future_primary_limit,
                reservation.frame_reservation(),
            )
        };
        if let (Some(durations), Some(started)) =
            (phase_durations.as_deref_mut(), slot_acquisition_started)
        {
            durations.render_target_preparation_ns = elapsed_nanos(started);
        }
        let slot = slot_result?;
        let (frame_id, pool_generation) = {
            let swapchain = self.swapchain()?;
            (swapchain.next_frame_id(), swapchain.pool_generation())
        };
        let framebuffer_started = phase_durations.as_ref().map(|_| Instant::now());
        let framebuffer_result = self.framebuffer(slot);
        if let (Some(durations), Some(started)) =
            (phase_durations.as_deref_mut(), framebuffer_started)
        {
            durations.render_target_preparation_ns = durations
                .render_target_preparation_ns
                .saturating_add(elapsed_nanos(started));
        }
        let framebuffer_id = match framebuffer_result {
            Ok(framebuffer) => framebuffer.get(),
            Err(error) => {
                self.swapchain_mut()?.cancel_render_before_gpu(slot)?;
                return Err(error);
            }
        };
        let mut presentation_samples = resolved_scene
            .surfaces
            .iter()
            .filter_map(|surface| server.presentation_commit_key_for_renderable_surface(surface))
            .collect::<Vec<_>>();
        let hardware_cursor_presentation_key = match frozen_cursor_plan.delivery {
            crate::native_output::presentation::plane::PresentedCursorDelivery::Hardware => {
                frozen_cursor_plane_owner
                    .as_ref()
                    .and_then(|owner| owner.client_source_key)
                    .and_then(|source_key| {
                        server.presentation_commit_key_for_surface_commit(
                            source_key.surface_id,
                            oblivion_one::compositor::SurfaceCommitSequence(
                                source_key.commit_sequence,
                            ),
                        )
                    })
            }
            crate::native_output::presentation::plane::PresentedCursorDelivery::Software
            | crate::native_output::presentation::plane::PresentedCursorDelivery::Hidden => None,
        };
        let exact_cursor_commit = match frozen_cursor_plan.delivery {
            crate::native_output::presentation::plane::PresentedCursorDelivery::Software => server
                .client_cursor_render_state()
                .map(|cursor| (cursor.surface.surface_id, cursor.surface.commit_sequence)),
            crate::native_output::presentation::plane::PresentedCursorDelivery::Hardware => {
                hardware_cursor_presentation_key.map(|key| (key.surface_id, key.commit_sequence))
            }
            crate::native_output::presentation::plane::PresentedCursorDelivery::Hidden => None,
        };
        if frozen_cursor_plan.delivery
            == crate::native_output::presentation::plane::PresentedCursorDelivery::Software
            && let Some((surface_id, commit_sequence)) = exact_cursor_commit
            && let Some(key) =
                server.presentation_commit_key_for_surface_commit(surface_id, commit_sequence)
        {
            presentation_samples.push(key);
        }
        let protocol_batch_id = server
            .take_frame_batch_for_render_with_presentation_samples(frame_id, presentation_samples);
        let transaction_id = match output_transactions.allocate_id() {
            Ok(transaction_id) => transaction_id,
            Err(error) => {
                server.restore_frame_batch_after_render_failure(protocol_batch_id);
                self.swapchain_mut()?.cancel_render_before_gpu(slot)?;
                return Err(io::Error::other(error));
            }
        };
        let prepare_intent = if render_ahead {
            Some(O1PrepareIntent::from_target(
                reservation.fixed_target().ok_or_else(|| {
                    io::Error::other("variable composited reservation cannot prepare O1")
                })?,
                output_generation,
                self.swapchain()
                    .ok()
                    .and_then(AtomicOutputSwapchain::deferred_o1_predecessor)
                    .ok_or_else(|| {
                        io::Error::other("predictive render-ahead has no exact predecessor")
                    })?,
            ))
        } else {
            None
        };
        let transaction_created_at = MonotonicTimestampNs::new(monotonic_now_ns()?);
        let cursor_visible = matches!(
            &cursor,
            Some(CursorPlaneAssignment::Atomic {
                state: Some(state),
                ..
            }) if state.visible
        );
        let transaction_result = if let Some(intent) = prepare_intent {
            OutputTransaction::composited_deferred_o1(
                self.direct.output_id,
                transaction_id,
                output_generation,
                transaction_created_at,
                intent,
                pacing_mode,
                frame_id,
                render_generation,
                pool_generation,
                slot,
                framebuffer_id,
                cursor,
                protocol_batch_id,
            )
        } else if let Some(target) = reservation.fixed_target() {
            OutputTransaction::composited_with_direct_equivalence(
                self.direct.output_id,
                transaction_id,
                output_generation,
                transaction_created_at,
                target,
                pacing_mode,
                frame_id,
                render_generation,
                pool_generation,
                slot,
                framebuffer_id,
                cursor,
                protocol_batch_id,
                equivalent_direct_key,
            )
        } else if let PreparedCompositedReservation::Variable { reservation } = reservation {
            if render_ahead || o1_admission.is_some() {
                return Err(io::Error::other(
                    "variable composited reservation carried O1 admission state",
                ));
            }
            OutputTransaction::composited_variable_with_direct_equivalence(
                self.direct.output_id,
                transaction_id,
                output_generation,
                transaction_created_at,
                reservation,
                content_type,
                NativeOutputPacingMode::ReactiveDouble,
                frame_id,
                render_generation,
                pool_generation,
                slot,
                framebuffer_id,
                cursor,
                protocol_batch_id,
                equivalent_direct_key,
            )
        } else {
            unreachable!("fixed composited reservation exposes its target")
        };
        let transaction = match transaction_result.and_then(|transaction| {
            transaction.with_presentation_state(presentation_mode, content_type)
        }) {
            Ok(transaction) => transaction
                .with_presentation_validation_key(presentation_validation_key)
                .with_client_cursor_presentation_key(hardware_cursor_presentation_key),
            Err(error) => {
                server.restore_frame_batch_after_render_failure(protocol_batch_id);
                self.swapchain_mut()?.cancel_render_before_gpu(slot)?;
                return Err(io::Error::other(error));
            }
        };
        let composite_started_at = MonotonicTimestampNs::new(monotonic_now_ns()?);
        if let Err(error) = output_transactions.insert(transaction) {
            server.restore_frame_batch_after_render_failure(protocol_batch_id);
            self.swapchain_mut()?.cancel_render_before_gpu(slot)?;
            return Err(io::Error::other(error));
        }
        if let Some(batch_id) = take_client_cursor_presentation_feedback_batch(
            server,
            hardware_cursor_presentation_key,
            frozen_cursor_plan.delivery,
            cursor_visible,
        ) {
            if let Err(error) =
                output_transactions.attach_presentation_feedback_batch(transaction_id, batch_id)
            {
                server.restore_presentation_feedback_batch_after_failure(batch_id);
                settle_failed_output_transaction(
                    output_transactions,
                    transaction_id,
                    OutputTransactionFailureStage::RenderPreparation,
                    transaction_created_at,
                    |obligations| {
                        server.restore_frame_batch_after_render_failure(
                            obligations
                                .frame_batch_id()
                                .expect("composited transaction frame"),
                        );
                        self.swapchain_mut()?.cancel_render_before_gpu(slot)?;
                        Ok(())
                    },
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                return Err(io::Error::other(error));
            }
        }
        server.trace_surface_pipeline_surfaces(
            SurfacePipelineEvent::SceneResolved,
            resolved_scene.surfaces.iter().map(|surface| {
                (
                    surface.surface_id,
                    surface.commit_sequence,
                    Some(surface.buffer_id().get()),
                )
            }),
            Some(protocol_batch_id.get()),
            Some(transaction_id.get()),
            Some(frame_id),
        );
        // This is the estimator's production render boundary. Everything before it may
        // include protocol bookkeeping or diagnostics; everything after it is explicit
        // scene encoding, fence export, and GPU work owned by this output frame.
        let callback_timing = server.frame_callback_timing_for_batch(protocol_batch_id);
        let mut gpu_sampling_started = false;
        let resolved_snapshot = resolved_scene.snapshot_owned();
        let resolved_scene_signature = resolved_scene.scene_identity_signature();
        let mut sampled_surface_ids = resolved_scene.surface_ids().collect::<Vec<_>>();
        if frozen_cursor_plan.delivery
            == crate::native_output::presentation::plane::PresentedCursorDelivery::Hardware
            && let Some(source_key) = frozen_cursor_plane_owner
                .as_ref()
                .and_then(|owner| owner.client_source_key)
        {
            sampled_surface_ids.push(source_key.surface_id);
        }
        let surface_damage = server.capture_surface_damage_presentation_for_surface_ids_and_commit(
            sampled_surface_ids,
            exact_cursor_commit.or_else(|| {
                frozen_cursor_plane_owner
                    .as_ref()
                    .and_then(|owner| owner.client_source_key)
                    .map(|source_key| {
                        (
                            source_key.surface_id,
                            oblivion_one::compositor::SurfaceCommitSequence(
                                source_key.commit_sequence,
                            ),
                        )
                    })
            }),
        );
        let hardware_cursor_surface_id = (frozen_cursor_plan.delivery
            == crate::native_output::presentation::plane::PresentedCursorDelivery::Hardware)
            .then(|| {
                frozen_cursor_plane_owner
                    .as_ref()
                    .and_then(|owner| owner.client_source_key)
                    .map(|source_key| source_key.surface_id)
            })
            .flatten();
        let cursor_surface_id = match frozen_cursor_plan.delivery {
            crate::native_output::presentation::plane::PresentedCursorDelivery::Software => {
                exact_cursor_commit.map(|(surface_id, _)| surface_id)
            }
            crate::native_output::presentation::plane::PresentedCursorDelivery::Hardware => {
                hardware_cursor_surface_id
            }
            crate::native_output::presentation::plane::PresentedCursorDelivery::Hidden => None,
        };
        if let Some(surface_id) = cursor_surface_id
            && surface_damage.contains_surface_id(surface_id)
        {
            server.note_client_cursor_surface_sample(
                frozen_cursor_plan.delivery
                    == crate::native_output::presentation::plane::PresentedCursorDelivery::Hardware,
            );
        }
        let render_snapshot_freeze_started = phase_durations.as_ref().map(|_| Instant::now());
        let mut frozen_render_frame = FrozenNativeRenderFrame::freeze(
            self.width,
            self.height,
            resolved_scene,
            &*server,
            input_state,
            cursor_mode,
            Some(render_damage.as_renderer_damage(self.width, self.height)),
        );
        if let (Some(durations), Some(started)) = (
            phase_durations.as_deref_mut(),
            render_snapshot_freeze_started,
        ) {
            durations.render_snapshot_freeze_ns = elapsed_nanos(started);
        }
        let context = AtomicRenderSettlementContext {
            slot,
            framebuffer_id,
            frame_id,
            transaction_id,
            protocol_batch_id,
            output_generation,
            render_generation,
            pool_generation,
            reservation,
            prepare_intent,
            o1_admission,
            surface_damage,
            composite_started_at,
            callback_timing,
            hardware_cursor_surface_id,
            frozen_cursor_plan,
            frozen_cursor_plane_owner,
            frozen_cursor_trace_reveal,
            resolved_snapshot,
            resolved_scene_signature,
            resolved_render_generation: frozen_render_frame.scene.render_generation,
            presentation_state_transition_required,
            dmabuf_gpu_release_safety,
            dmabuf_gpu_release_lease_id,
            render_target: None,
        };
        let mut context = context;
        let target_preparation_started = phase_durations.as_ref().map(|_| Instant::now());
        let target_request = self.render_target_request(context.slot);
        if let (Some(durations), Some(started)) =
            (phase_durations.as_deref_mut(), target_preparation_started)
        {
            durations.render_target_preparation_ns = durations
                .render_target_preparation_ns
                .saturating_add(elapsed_nanos(started));
        }
        context.render_target = target_request.as_ref().ok().copied();
        if matches!(&self.render_transport, AtomicRenderTransport::Worker(_)) {
            let Some(target) = context.render_target else {
                let error = target_request.expect_err("missing target preserves its error");
                return self.settle_render_completion(
                    context,
                    frozen_render_frame,
                    Err(error),
                    false,
                    None,
                    server,
                    output_transactions,
                );
            };
            let (controls_before, control_epoch) = self.take_render_controls_for_job()?;
            let job_id = RenderWorkerJobId::new(transaction_id.get())
                .ok_or_else(|| io::Error::other("output transaction ID is zero"))?;
            let dispatch_at_ns = monotonic_now_ns()?;
            let need_no_visual_release_fence = context.dmabuf_gpu_release_lease_id.is_some()
                && server.frame_batch_dmabuf_release_count(protocol_batch_id) > 0
                && context
                    .dmabuf_gpu_release_safety
                    .permits_compositor_gpu_release();
            let identity = AtomicRenderWorkerIdentity {
                job_id,
                frame_id,
                transaction_id,
                output_generation,
                render_generation,
                control_epoch,
                target,
            };
            let job = AtomicRenderWorkerJob {
                job_id,
                frame_id,
                transaction_id,
                output_generation,
                render_generation,
                target,
                frozen_frame: frozen_render_frame,
                need_no_visual_release_fence,
                dispatch_at_ns,
                required_control_epoch: control_epoch,
                controls_before,
            };
            let dispatch = self
                .render_worker_mut()
                .ok_or_else(|| io::Error::other("worker transport disappeared"))?
                .try_dispatch_render(job);
            return match dispatch {
                Ok(()) => Ok(AtomicFrameRenderOutcome::WorkerDispatched { identity, context }),
                Err(job) => {
                    self.pending_render_controls = job.controls_before;
                    self.settle_render_completion(
                        context,
                        job.frozen_frame,
                        Err(io::Error::other(
                            "render worker rejected a serialized frame command",
                        )),
                        false,
                        None,
                        server,
                        output_transactions,
                    )
                }
            };
        }
        let render_outcome = if let Some(target) = context.render_target {
            self.render_to_slot(
                target,
                context.frame_id,
                &mut frozen_render_frame,
                &mut gpu_sampling_started,
                phase_durations.as_deref_mut(),
            )
        } else {
            Err(target_request.expect_err("missing render target retains its error"))
        };
        self.settle_render_completion(
            context,
            frozen_render_frame,
            render_outcome,
            gpu_sampling_started,
            None,
            server,
            output_transactions,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn settle_render_completion(
        &mut self,
        context: AtomicRenderSettlementContext,
        frozen_render_frame: FrozenNativeRenderFrame,
        render_outcome: io::Result<AtomicSlotRenderOutcome>,
        gpu_sampling_started: bool,
        release_fence_from_worker: Option<NativeRenderFence>,
        server: &mut OwnCompositorServer,
        output_transactions: &mut OutputTransactionLedger,
    ) -> io::Result<AtomicFrameRenderOutcome> {
        let AtomicRenderSettlementContext {
            slot,
            framebuffer_id,
            frame_id,
            transaction_id,
            protocol_batch_id,
            output_generation,
            render_generation,
            pool_generation,
            reservation,
            prepare_intent,
            o1_admission,
            surface_damage,
            composite_started_at,
            callback_timing,
            hardware_cursor_surface_id,
            frozen_cursor_plan,
            frozen_cursor_plane_owner,
            frozen_cursor_trace_reveal,
            resolved_snapshot,
            resolved_scene_signature,
            resolved_render_generation,
            presentation_state_transition_required,
            dmabuf_gpu_release_safety,
            dmabuf_gpu_release_lease_id,
            render_target: _,
        } = context;
        let resolved_scene = &frozen_render_frame.scene;
        let parts = match render_outcome {
            Ok(AtomicSlotRenderOutcome::Rendered(parts)) => *parts,
            Ok(AtomicSlotRenderOutcome::Skipped {
                slot,
                reason,
                render_us,
            }) => {
                if presentation_state_transition_required {
                    if gpu_sampling_started {
                        // A renderer skip can still follow partial GLES work.
                        // Complete it before restoring any sampled client
                        // buffers or recycling this compositor slot.
                        self.finish_render_work();
                    }
                    settle_failed_output_transaction(
                        output_transactions,
                        transaction_id,
                        if gpu_sampling_started {
                            OutputTransactionFailureStage::RenderExecution
                        } else {
                            OutputTransactionFailureStage::RenderPreparation
                        },
                        MonotonicTimestampNs::new(monotonic_now_ns()?),
                        |obligations| {
                            restore_presentation_feedback_obligation(server, obligations);
                            let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                                io::Error::other(
                                    "presentation-state retry transaction has no frame batch",
                                )
                            })?;
                            server.set_frame_batch_surface_damage(batch_id, surface_damage);
                            server.restore_frame_batch_after_render_failure(batch_id);
                            if gpu_sampling_started {
                                self.complete_unpresented_render(slot)?;
                            } else {
                                self.swapchain_mut()?.cancel_render_before_gpu(slot)?;
                            }
                            Ok(())
                        },
                    )
                    .map_err(|error| io::Error::other(error.to_string()))?;
                    return Ok(AtomicFrameRenderOutcome::RetryPresentationState { render_us });
                }
                let release_fence = if dmabuf_gpu_release_lease_id.is_some()
                    && server.frame_batch_dmabuf_release_count(protocol_batch_id) > 0
                    && dmabuf_gpu_release_safety.permits_compositor_gpu_release()
                {
                    release_fence_from_worker.or_else(|| self.create_render_fence().ok())
                } else {
                    None
                };
                let settle_result = settle_no_visual_change_output_transaction(
                    output_transactions,
                    transaction_id,
                    MonotonicTimestampNs::new(monotonic_now_ns()?),
                    |obligations| {
                        let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                            io::Error::other("skipped render transaction has no frame batch")
                        })?;
                        self.swapchain_mut()?.cancel_render_before_gpu(slot)?;
                        restore_presentation_feedback_obligation(server, obligations);
                        server.set_frame_batch_surface_damage(batch_id, surface_damage);
                        if let (Some(lease_id), Some(_)) =
                            (dmabuf_gpu_release_lease_id, release_fence.as_ref())
                        {
                            server.transfer_frame_batch_dmabuf_releases_to_pending_gpu_lease(
                                batch_id, lease_id,
                            );
                        }
                        server.complete_no_visual_change_frame_batch(batch_id);
                        Ok(())
                    },
                )
                .map_err(|error| io::Error::other(error.to_string()));
                if let Err(error) = settle_result {
                    if let Some(lease_id) = dmabuf_gpu_release_lease_id {
                        server.requeue_dmabuf_gpu_release_lease(lease_id);
                    }
                    return Err(error);
                }
                return Ok(AtomicFrameRenderOutcome::Skipped {
                    reason,
                    render_us,
                    dmabuf_gpu_release: dmabuf_gpu_release_lease_id.zip(release_fence),
                });
            }
            Ok(AtomicSlotRenderOutcome::LifecycleFallback {
                fallbacks,
                render_us,
            }) => {
                // The lifecycle renderer may have sampled client dmabufs or
                // partially written the acquired scanout framebuffer before
                // reporting a recoverable representation fallback. If GLES
                // work started, complete it before restoring frame ownership
                // or recycling the slot.
                if gpu_sampling_started {
                    self.finish_render_work();
                }
                settle_failed_output_transaction(
                    output_transactions,
                    transaction_id,
                    OutputTransactionFailureStage::RenderExecution,
                    MonotonicTimestampNs::new(monotonic_now_ns()?),
                    |obligations| {
                        restore_presentation_feedback_obligation(server, obligations);
                        let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                            io::Error::other("lifecycle fallback transaction has no frame batch")
                        })?;
                        server.restore_frame_batch_after_render_failure(batch_id);
                        self.complete_unpresented_render(slot)?;
                        Ok(())
                    },
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                return Ok(AtomicFrameRenderOutcome::LifecycleFallback {
                    fallbacks,
                    render_us,
                });
            }
            Err(error) => {
                if gpu_sampling_started {
                    self.finish_render_work();
                }
                let failure_stage = if gpu_sampling_started {
                    OutputTransactionFailureStage::RenderExecution
                } else {
                    OutputTransactionFailureStage::RenderPreparation
                };
                settle_failed_output_transaction(
                    output_transactions,
                    transaction_id,
                    failure_stage,
                    MonotonicTimestampNs::new(monotonic_now_ns()?),
                    |obligations| {
                        if gpu_sampling_started {
                            discard_presentation_feedback_obligation(server, obligations);
                        } else {
                            restore_presentation_feedback_obligation(server, obligations);
                        }
                        let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                            io::Error::other("render failure transaction has no frame batch")
                        })?;
                        if gpu_sampling_started {
                            server.discard_frame_batch(
                                batch_id,
                                FrameBatchDiscardReason::FatalOutputFailure,
                            );
                            let _ = self.swapchain_mut()?.quarantine_rendering(
                                None,
                                OutputQuarantineReason::PostDrawRenderFailure,
                            );
                        } else {
                            server.restore_frame_batch_after_render_failure(batch_id);
                            self.swapchain_mut()?.cancel_render_before_gpu(slot)?;
                        }
                        server.cancel_lifecycle_precapture_after_render_failure();
                        Ok(())
                    },
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                return Err(error);
            }
        };
        let render_us = parts.render_us;
        let lifecycle_snapshot =
            oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot::qualified_from_sample(
                &resolved_scene.lifecycle,
                &parts.lifecycle_evidence,
            );
        server.trace_surface_pipeline_surfaces(
            SurfacePipelineEvent::SceneSampled,
            resolved_scene.surfaces.iter().map(|surface| {
                (
                    surface.surface_id,
                    surface.commit_sequence,
                    Some(surface.buffer_id().get()),
                )
            }),
            Some(protocol_batch_id.get()),
            Some(transaction_id.get()),
            Some(frame_id),
        );
        server.trace_surface_pipeline_surfaces(
            SurfacePipelineEvent::OutputFrameRendered,
            resolved_scene.surfaces.iter().map(|surface| {
                (
                    surface.surface_id,
                    surface.commit_sequence,
                    Some(surface.buffer_id().get()),
                )
            }),
            Some(protocol_batch_id.get()),
            Some(transaction_id.get()),
            Some(frame_id),
        );
        let repaint_stats = parts.stats;
        let render_damage_signature = parts
            .scene_commit
            .repaint_plan()
            .render_damage
            .identity_signature();
        let repair_damage_signature = parts
            .scene_commit
            .repaint_plan()
            .repair_damage
            .identity_signature();
        let rendered_at = MonotonicTimestampNs::new(monotonic_now_ns()?);
        let frame = RenderedOutputFrame {
            output_id: self.direct.output_id,
            id: frame_id,
            transaction_id,
            slot,
            framebuffer_id: FramebufferId::new(framebuffer_id)
                .ok_or_else(|| io::Error::other("atomic output framebuffer ID is zero"))?,
            render_generation,
            pool_generation,
            reservation: prepare_intent.map_or(
                reservation.frame_reservation(),
                crate::native_output::presentation::transaction::FramePresentationReservation::DeferredO1,
            ),
            presentation_timing: reservation.presentation_timing(rendered_at),
            render_fence: parts.render_fence,
            fence_timing_evidence: None,
            scene_commit: parts.scene_commit,
            surface_damage,
            protocol_batch_id,
            composite_started_at,
            fence_exported_at: rendered_at,
            rendered_at,
            client_commit_ns: callback_timing.map(|timing| timing.commit_ns),
            callback_reaction_ns: callback_timing.and_then(|timing| timing.reaction_ns),
            callback_admission_ns: callback_timing.and_then(|timing| timing.admission_ns),
            callback_surface_id: callback_timing.map(|timing| timing.surface_id),
            hardware_cursor_surface_id,
            cpu_prepass_duration_ns: 0,
            cpu_encode_duration_ns: parts.render_us.saturating_mul(1_000),
            frozen_cursor_plan,
            frozen_cursor_plane_owner,
            frozen_cursor_trace_reveal,
            o1_admission,
        };
        let framebuffer_slot = frame.slot.get();
        match self.swapchain_mut()?.finish_render_owned(frame) {
            Ok(frame_id) => {
                if prepare_intent.is_some() {
                    output_transactions.mark_ready_unbound(transaction_id, rendered_at)
                } else {
                    output_transactions.mark_ready(transaction_id, rendered_at)
                }
                .map_err(io::Error::other)?;
                let (deferred_o1_binding_advanced_intervals, deferred_o1_binding_failure) =
                    if prepare_intent.is_some() {
                        match self.bind_ready_deferred_o1(
                            output_transactions,
                            output_generation,
                            rendered_at,
                        )? {
                            DeferredO1BindingResult::NotReady
                            | DeferredO1BindingResult::WaitingForPredecessor => (None, None),
                            DeferredO1BindingResult::Bound { advanced_intervals } => {
                                (Some(advanced_intervals), None)
                            }
                            DeferredO1BindingResult::Stale(failure) => (None, Some(failure)),
                        }
                    } else {
                        (None, None)
                    };
                if let Some(sample) = resolved_scene.workspace_presentation_sample.as_ref()
                    && sample.mathematically_settled
                {
                    server.retire_workspace_presentation_if_rendered(sample);
                }
                server.mark_frame_callbacks_rendered(protocol_batch_id);
                Ok(AtomicFrameRenderOutcome::Rendered {
                    frame_id,
                    transaction_id,
                    protocol_batch_id,
                    render_us,
                    repaint_stats,
                    lifecycle_snapshot,
                    resolved_snapshot,
                    resolved_scene_signature,
                    render_damage_signature,
                    repair_damage_signature,
                    resolved_render_generation,
                    framebuffer_slot,
                    deferred_o1_binding_advanced_intervals,
                    deferred_o1_binding_failure,
                })
            }
            Err(error) => {
                settle_failed_output_transaction(
                    output_transactions,
                    transaction_id,
                    OutputTransactionFailureStage::RenderExecution,
                    MonotonicTimestampNs::new(monotonic_now_ns()?),
                    |obligations| {
                        discard_presentation_feedback_obligation(server, obligations);
                        let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                            io::Error::other("ready ownership failure has no frame batch")
                        })?;
                        server.discard_frame_batch(
                            batch_id,
                            FrameBatchDiscardReason::FatalOutputFailure,
                        );
                        Ok(())
                    },
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                Err(error)
            }
        }
    }

    pub(crate) fn complete_pageflip(
        &mut self,
        token: PageFlipToken,
        // Explicit slot age is indexed by confirmed presentation serial, so
        // this transition must be prepared from actual pageflip predecessor
        // snapshots rather than the frame's render-time repair damage.
        presented_transition_damage: OutputDamage,
    ) -> io::Result<CompositedPageflipCompletion> {
        let generation = self.swapchain()?.pool_generation();
        let completed = self.swapchain_mut()?.complete_pageflip(token, generation)?;
        let mut completed_frame = completed.frame;
        let timing_result = monotonic_now_ns().and_then(|observed_at| {
            completed_frame.sample_fence_timing(MonotonicTimestampNs::new(observed_at))
        });
        let fence_timing_accounted = timing_result
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .is_some_and(|evidence| evidence.render_sample_recorded);
        let physical_identity = OutputFrameIdentitySnapshot::from(&completed_frame);
        let RenderedOutputFrame {
            id,
            transaction_id,
            reservation,
            presentation_timing,
            scene_commit,
            surface_damage,
            protocol_batch_id,
            composite_started_at,
            rendered_at,
            client_commit_ns,
            callback_reaction_ns,
            callback_admission_ns,
            callback_surface_id,
            hardware_cursor_surface_id,
            o1_admission,
            ..
        } = completed_frame;
        if matches!(reservation, FramePresentationReservation::DeferredO1(_)) {
            return Err(io::Error::other(
                "deferred O1 frame cannot reach pageflip completion before binding",
            ));
        }
        match (reservation, presentation_timing) {
            (
                FramePresentationReservation::Fixed(_)
                | FramePresentationReservation::DeferredO1(_),
                KmsPresentationTiming::Fixed(_),
            )
            | (FramePresentationReservation::Variable(_), KmsPresentationTiming::Variable { .. }) =>
                {}
            _ => return Err(io::Error::other("pageflip frame timing domain is invalid")),
        }
        let render_control_result =
            self.queue_render_control(AtomicRenderControl::CommitPresented {
                frame: scene_commit,
                presented_transition_damage,
            });
        let (fence_signal, timing_error) = complete_confirmed_pageflip_with_timing(
            timing_result
                .map(|sample| sample.map(|evidence| (evidence.signaled_at, evidence.quality))),
            || {
                if let Some(pool) = self.physical_pool.as_mut() {
                    pool.physical_slot_mut(completed.new_current)
                        .expect("validated pageflip slot remains in physical pool")
                        .record_presentation(completed.presentation_serial);
                }
            },
        );
        if let Err(error) = render_control_result {
            return Err(error);
        }
        if let Some(error) = timing_error {
            self.counters.render_fence_timing_unavailable += 1;
            eprintln!(
                "native render-fence timing unavailable after confirmed pageflip for frame {id}: {error}"
            );
        }
        Ok(CompositedPageflipCompletion {
            presented: PresentedOutputFrame {
                frame_id: id,
                physical_identity,
                transaction_id,
                reservation,
                presentation_timing,
                composite_started_at,
                rendered_at,
                submit_started_at: completed.submit_started_at,
                submit_returned_at: completed.submit_returned_at,
                fence_signal,
                fence_timing_accounted,
                o1_admission,
                client_commit_ns,
                callback_reaction_ns,
                callback_admission_ns,
                callback_surface_id,
                callback_surface_is_exclusive: callback_surface_id.is_some_and(|surface_id| {
                    surface_damage
                        .is_exclusive_surface_id_excluding(surface_id, hardware_cursor_surface_id)
                }),
            },
            protocol_batch_id,
            surface_damage,
        })
    }

    pub(crate) fn pending_timing_fd(&self) -> Option<RawFd> {
        self.swapchain.as_ref()?.pending_timing_fd()
    }

    pub(crate) fn bind_ready_deferred_o1(
        &mut self,
        output_transactions: &mut OutputTransactionLedger,
        output_generation: u64,
        bind_at: MonotonicTimestampNs,
    ) -> io::Result<DeferredO1BindingResult> {
        let readiness = self
            .swapchain()?
            .deferred_o1_binding_readiness(output_generation);
        match readiness {
            DeferredO1BindingReadiness::NotDeferred => {
                return Ok(DeferredO1BindingResult::NotReady);
            }
            DeferredO1BindingReadiness::WaitingForPredecessor => {
                return Ok(DeferredO1BindingResult::WaitingForPredecessor);
            }
            DeferredO1BindingReadiness::Stale(failure) => {
                return Ok(DeferredO1BindingResult::Stale(failure));
            }
            DeferredO1BindingReadiness::Bindable { .. } => {}
        }
        let Some((transaction_id, target, submit_window, advanced_intervals)) =
            self.swapchain()?
                .deferred_o1_binding_candidate_for_readiness(readiness, bind_at)?
        else {
            return Ok(DeferredO1BindingResult::NotReady);
        };
        output_transactions
            .bind_deferred_o1(transaction_id, target)
            .map_err(io::Error::other)?;
        self.swapchain_mut()?
            .commit_deferred_o1_binding(transaction_id, target, submit_window)?;
        Ok(DeferredO1BindingResult::Bound { advanced_intervals })
    }

    pub(crate) fn ready_render_fence_is_signaled(&self) -> io::Result<bool> {
        self.swapchain
            .as_ref()
            .ok_or_else(|| io::Error::other("explicit output swapchain is unavailable"))?
            .ready_render_fence_is_signaled()
    }

    pub(crate) fn ready_render_fence_fd(&self) -> Option<RawFd> {
        self.swapchain.as_ref()?.ready_render_fence_fd()
    }

    pub(crate) const fn counters(&self) -> ExplicitOutputCounters {
        self.counters
    }

    pub(crate) fn sample_pending_timing(
        &mut self,
        observed_at: MonotonicTimestampNs,
    ) -> io::Result<Option<PendingFenceTiming>> {
        let Some(frame) = self.swapchain_mut()?.pending_frame_mut() else {
            return Ok(None);
        };
        let Some(evidence) = frame.sample_fence_timing(observed_at)? else {
            return Ok(None);
        };
        let timing = PendingFenceTiming {
            frame_id: frame.id,
            reservation: frame.reservation,
            presentation_timing: frame.presentation_timing,
            composite_started_at: frame.composite_started_at,
            signaled_at: evidence.signaled_at,
            quality: evidence.quality,
        };
        frame.mark_fence_timing_accounted();
        drop(frame.render_fence.take_timing_fd());
        Ok(Some(timing))
    }

    fn discard_failed_frame_resources(&mut self, frame: RenderedOutputFrame) {
        let _ = self.queue_render_control(AtomicRenderControl::DiscardRendered {
            frame: frame.scene_commit,
        });
        drop(frame.surface_damage);
    }

    pub(crate) fn promote_initial_presented(
        &mut self,
        slot: OutputSlotId,
        scene_commit: EglSceneFrameCommit,
    ) -> io::Result<()> {
        let pool = self
            .physical_pool
            .as_mut()
            .ok_or_else(|| io::Error::other("explicit output pool is unavailable"))?;
        let slots = OutputSlotSet::new([
            OutputSlotId::new(0).unwrap(),
            OutputSlotId::new(1).unwrap(),
            OutputSlotId::new(2).unwrap(),
        ])?;
        let framebuffer_id = pool.physical_slot(slot)?.framebuffer;
        let mut swapchain = AtomicOutputSwapchain::from_presented_slots_for_output(
            self.direct.output_id,
            slots,
            slot,
            pool.pool_generation(),
        )?;
        swapchain.set_current_framebuffer_id(framebuffer_id);
        self.swapchain = Some(swapchain);
        pool.physical_slot_mut(slot)?.record_presentation(0);
        let _ = self.queue_render_control(AtomicRenderControl::CommitPresented {
            frame: scene_commit,
            presented_transition_damage: OutputDamage::Full,
        });
        self.direct.inhibit_until_composited_present = false;
        Ok(())
    }

    pub(crate) fn swapchain(&self) -> io::Result<&AtomicOutputSwapchain> {
        self.swapchain
            .as_ref()
            .ok_or_else(|| io::Error::other("explicit output swapchain is not presented"))
    }

    pub(crate) fn swapchain_mut(&mut self) -> io::Result<&mut AtomicOutputSwapchain> {
        self.swapchain
            .as_mut()
            .ok_or_else(|| io::Error::other("explicit output swapchain is not presented"))
    }

    pub(crate) fn suspended_fence_fd(&self) -> io::Result<Option<std::os::fd::RawFd>> {
        self.swapchain()
            .and_then(AtomicOutputSwapchain::suspended_fence_fd)
    }

    pub(crate) fn recover_suspended_ready_if_signaled(&mut self) -> io::Result<bool> {
        if !self.swapchain()?.has_suspended_frame() {
            return Ok(false);
        }
        if !self.swapchain()?.suspended_fences_signaled()? {
            return Ok(false);
        }
        let mut recovered = false;
        while let Some(frame) = self.swapchain_mut()?.take_suspended_frame() {
            recovered = true;
            let _ = self.queue_render_control(AtomicRenderControl::DiscardRendered {
                frame: frame.scene_commit,
            });
            drop(frame.surface_damage);
        }
        self.swapchain_mut()?.recover_suspended_slot(true)?;
        Ok(recovered)
    }

    pub(crate) fn note_physical_primary_presentation(
        &mut self,
        claim: PrimaryRefreshClaim,
    ) -> io::Result<()> {
        self.swapchain_mut()?
            .note_physical_primary_presentation(claim)
    }

    pub(crate) fn revalidate_physical_primary_presentation(
        &self,
        claim: PrimaryRefreshClaim,
    ) -> io::Result<PhysicalPrimaryClaimRevalidation> {
        Ok(self
            .swapchain()?
            .revalidate_physical_primary_presentation(claim))
    }

    pub(crate) fn presented_direct_ownership(&self) -> &DirectPrimaryOwnership {
        &self.direct.ownership
    }

    pub(crate) fn has_live_direct_kms_ownership(&self) -> bool {
        let ownership = &self.direct.ownership;
        ownership.submitted.is_some()
            || ownership.presented.is_some()
            || !ownership.suspended.is_empty()
    }

    pub(crate) fn dmabuf_feedback(&self) -> EglGlesDmabufFeedback {
        self.dmabuf_feedback.clone()
    }

    pub(crate) const fn dmabuf_main_device(&self) -> Option<u64> {
        self.dmabuf_main_device
    }

    pub(crate) fn dmabuf_main_device_path(&self) -> Option<String> {
        self.dmabuf_main_device_path.clone()
    }

    pub(crate) fn dmabuf_egl_vendor(&self) -> &str {
        &self.dmabuf_egl_vendor
    }

    pub(crate) fn dmabuf_scanout_capabilities(&self) -> Option<DirectScanoutFeedbackCapabilities> {
        Some(self.dmabuf_scanout_capabilities.clone())
    }

    pub(crate) fn dmabuf_kms_preferred_state(&self) -> DmabufKmsPreferredState {
        self.dmabuf_kms_preferred_state
    }

    pub(crate) fn disarm_drm_cleanup(&mut self) {
        self.drm_cleanup_armed = false;
        self.direct.disarm_drm_cleanup();
    }

    fn physical_slot(&self, slot: OutputSlotId) -> io::Result<&AtomicOutputSlotBacking> {
        let pool = self
            .physical_pool
            .as_ref()
            .ok_or_else(|| io::Error::other("explicit output pool is unavailable"))?;
        pool.physical_slot(slot)
    }

    fn physical_slot_mut(
        &mut self,
        slot: OutputSlotId,
    ) -> io::Result<&mut AtomicOutputSlotBacking> {
        let pool = self
            .physical_pool
            .as_mut()
            .ok_or_else(|| io::Error::other("explicit output pool is unavailable"))?;
        pool.physical_slot_mut(slot)
    }
}

fn complete_confirmed_pageflip_with_timing<T>(
    timing: io::Result<Option<T>>,
    complete_ownership: impl FnOnce(),
) -> (Option<T>, Option<io::Error>) {
    complete_ownership();
    match timing {
        Ok(timing) => (timing, None),
        Err(error) => (None, Some(error)),
    }
}

#[expect(
    clippy::large_enum_variant,
    reason = "the rendered result carries the complete scene identity and repaint evidence"
)]
pub(crate) enum AtomicFrameRenderOutcome {
    WorkerDispatched {
        identity: AtomicRenderWorkerIdentity,
        context: AtomicRenderSettlementContext,
    },
    Rendered {
        frame_id: u64,
        transaction_id: OutputTransactionId,
        protocol_batch_id: CompositorFrameBatchId,
        render_us: u64,
        repaint_stats: GlesSceneFrameStats,
        resolved_snapshot: NativeSceneSnapshot,
        resolved_scene_signature: u64,
        render_damage_signature: u64,
        repair_damage_signature: u64,
        resolved_render_generation: u64,
        framebuffer_slot: u8,
        deferred_o1_binding_advanced_intervals: Option<u64>,
        deferred_o1_binding_failure: Option<DeferredO1BindingFailure>,
        lifecycle_snapshot: oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot,
    },
    Skipped {
        reason: FrameSkipReason,
        render_us: u64,
        dmabuf_gpu_release: Option<(
            oblivion_one::compositor::DmabufGpuReleaseLeaseId,
            NativeRenderFence,
        )>,
    },
    RetryPresentationState {
        render_us: u64,
    },
    LifecycleFallback {
        fallbacks: LifecycleRenderFallbacks,
        render_us: u64,
    },
}

/// Reactor-owned evidence needed after the renderer returns. This contains no
/// references to live compositor, input, transaction-ledger, or swapchain
/// state, so it can be parked across an asynchronous render command.
#[allow(dead_code, reason = "Phase 2B.2 reactor continuation boundary")]
pub(crate) struct AtomicRenderSettlementContext {
    pub(crate) slot: OutputSlotId,
    pub(crate) framebuffer_id: u32,
    pub(crate) frame_id: u64,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) protocol_batch_id: CompositorFrameBatchId,
    pub(crate) output_generation: u64,
    pub(crate) render_generation: u64,
    pub(crate) pool_generation: u64,
    pub(crate) reservation: PreparedCompositedReservation,
    pub(crate) prepare_intent: Option<O1PrepareIntent>,
    pub(crate) o1_admission: Option<O1AdmissionObservation>,
    pub(crate) surface_damage: SurfaceDamagePresentation,
    pub(crate) composite_started_at: MonotonicTimestampNs,
    pub(crate) callback_timing: Option<oblivion_one::compositor::FrameCallbackTimingEvidence>,
    pub(crate) hardware_cursor_surface_id: Option<u32>,
    pub(crate) frozen_cursor_plan:
        crate::native_output::presentation::plane::FrozenPrimaryCursorPlan,
    pub(crate) frozen_cursor_plane_owner: Option<FrozenCursorPlaneOwner>,
    pub(crate) frozen_cursor_trace_reveal:
        Option<crate::native_output::presentation::plane::CursorRevealTraceSnapshot>,
    pub(crate) resolved_snapshot: NativeSceneSnapshot,
    pub(crate) resolved_scene_signature: u64,
    pub(crate) resolved_render_generation: u64,
    pub(crate) presentation_state_transition_required: bool,
    pub(crate) dmabuf_gpu_release_safety: DmabufGpuReleaseSafety,
    pub(crate) dmabuf_gpu_release_lease_id:
        Option<oblivion_one::compositor::DmabufGpuReleaseLeaseId>,
    pub(crate) render_target: Option<RenderTargetRequest>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingFenceTiming {
    pub(crate) frame_id: u64,
    pub(crate) reservation: FramePresentationReservation,
    pub(crate) presentation_timing: KmsPresentationTiming,
    pub(crate) composite_started_at: MonotonicTimestampNs,
    pub(crate) signaled_at: MonotonicTimestampNs,
    pub(crate) quality: FenceTimestampQuality,
}

#[derive(Debug)]
pub(crate) struct CompositedPageflipCompletion {
    pub(crate) presented: PresentedOutputFrame,
    pub(crate) protocol_batch_id: CompositorFrameBatchId,
    pub(crate) surface_damage: SurfaceDamagePresentation,
}

#[derive(Debug)]
pub(crate) struct PresentedOutputFrame {
    pub(crate) frame_id: u64,
    pub(crate) physical_identity: OutputFrameIdentitySnapshot,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) reservation: FramePresentationReservation,
    pub(crate) presentation_timing: KmsPresentationTiming,
    pub(crate) composite_started_at: MonotonicTimestampNs,
    pub(crate) rendered_at: MonotonicTimestampNs,
    pub(crate) submit_started_at: MonotonicTimestampNs,
    pub(crate) submit_returned_at: MonotonicTimestampNs,
    pub(crate) client_commit_ns: Option<u64>,
    pub(crate) callback_reaction_ns: Option<u64>,
    pub(crate) callback_admission_ns: Option<u64>,
    pub(crate) callback_surface_id: Option<u32>,
    pub(crate) callback_surface_is_exclusive: bool,
    pub(crate) fence_signal: Option<(MonotonicTimestampNs, FenceTimestampQuality)>,
    pub(crate) fence_timing_accounted: bool,
    pub(crate) o1_admission: Option<O1AdmissionObservation>,
}

impl AtomicRenderedFrameParts {
    pub(crate) fn paint_stats(&self, format: u32, width: u32, height: u32) -> NativePaintStats {
        NativePaintStats {
            backend: NativeScanoutKind::AtomicEglGbmExplicit,
            scanout_format: Some(format),
            width,
            height,
            bytes: 0,
            copy_bytes: 0,
            write_bytes: 0,
            gpu_draw_us: self.render_us,
            egl_swap_us: 0,
            shm_upload_bytes: self.stats.shm_upload_bytes,
            dmabuf_imports: self.stats.dmabuf_imports,
            dmabuf_reuses: self.stats.dmabuf_reuses,
            dmabuf_import_failures: self.stats.dmabuf_import_failures,
            dmabuf_cache_entries: self.stats.dmabuf_cache_entries,
            dmabuf_cache_peak_entries: self.stats.dmabuf_cache_peak_entries,
            dmabuf_cache_evictions: self.stats.dmabuf_cache_evictions,
            dmabuf_current_resource_reuses: self.stats.dmabuf_current_resource_reuses,
            dmabuf_cache_hits: self.stats.dmabuf_cache_hits,
            dmabuf_cache_misses: self.stats.dmabuf_cache_misses,
            dmabuf_cache_insertions: self.stats.dmabuf_cache_insertions,
            dmabuf_cache_evictions_dead: self.stats.dmabuf_cache_evictions_dead,
            dmabuf_cache_evictions_surface_bound: self.stats.dmabuf_cache_evictions_surface_bound,
            dmabuf_cache_evictions_surface_destroyed: self
                .stats
                .dmabuf_cache_evictions_surface_destroyed,
            dmabuf_cache_max_entries_for_one_surface: self
                .stats
                .dmabuf_cache_max_entries_for_one_surface,
            surface_resource_candidates: self.stats.surface_resource_candidates,
            surface_resource_consumers: self.stats.surface_resource_consumers,
            surface_resource_deferred: self.stats.surface_resource_deferred,
            shm_full_resyncs: self.stats.shm_full_resyncs,
            scene_rebuild: if self.stats.scene_rebuilt {
                DesktopSceneRebuildKind::Full
            } else {
                DesktopSceneRebuildKind::None
            },
            frame_copy: DesktopFrameCopyKind::None,
            total_us: self.render_us,
            render_us: self.render_us,
            copy_us: 0,
            write_us: 0,
            gles_repaint: Some(self.stats),
            swap_with_damage_used: false,
        }
    }
}

impl Drop for AtomicEglGbmScanout {
    fn drop(&mut self) {
        let render_domain_destroyed = match &mut self.render_transport {
            AtomicRenderTransport::Synchronous(domain) => {
                domain.teardown();
                true
            }
            AtomicRenderTransport::Worker(worker) => worker.shutdown_and_join().is_ok(),
        };
        if render_domain_destroyed {
            if let (Some(pool), Some(device)) =
                (self.physical_pool.take(), self._physical_device.as_ref())
            {
                // Revoked DRM skips rm_fb but still drops all GBM BOs. Render
                // imports have already been destroyed or acknowledged above.
                pool.teardown(device.as_fd(), self.drm_cleanup_armed);
            }
            self._physical_device.take();
        } else {
            // A failed worker join leaves GPU ownership uncertain. Retain the
            // BO and DRM device until process exit rather than risking reuse.
            if let Some(pool) = self.physical_pool.take() {
                std::mem::forget(pool);
            }
            if let Some(device) = self._physical_device.take() {
                std::mem::forget(device);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::time::Duration;

    fn adaptive_sync_validation_key() -> CompositedPresentationValidationKey {
        CompositedPresentationValidationKey::new(
            oblivion_one::core::OutputId::from_raw(1).expect("test output ID"),
            9,
            42,
            43,
            DrmFormatModifierPair {
                fourcc: 0x3432_5258,
                modifier: 0,
            },
            OutputPresentationMode::AdaptiveSync,
            0,
            false,
            None,
            false,
            false,
            true,
            DrmContentType::Graphics,
        )
    }

    fn prepared_adaptive_sync(
        validation_key: Option<CompositedPresentationValidationKey>,
        validation_accepted: bool,
        confirmed_state_matches: bool,
        presentation_state_transition_required: bool,
    ) -> PreparedCompositedPresentationState {
        PreparedCompositedPresentationState {
            mode: OutputPresentationMode::AdaptiveSync,
            content_type: DrmContentType::Graphics,
            validation_key,
            validation_accepted,
            confirmed_state_matches,
            presentation_state_transition_required,
            vrr_blocker: None,
            async_blocker: None,
            combination_blocker: None,
            acquire_strategy: 0,
        }
    }

    fn reactive_double_admission() -> VariableCompositedAdmission {
        VariableCompositedAdmission {
            reactive_double: true,
            desired_credit_one: true,
            render_ahead: false,
            fixed_target_owned: false,
            commit_timing_applies: false,
            deferred_o1: false,
        }
    }

    #[test]
    fn composited_variable_runtime_reservation_is_not_quantized_at_165_hz() {
        let anchor = MonotonicTimestampNs::new(1_000_000_000);
        let now = MonotonicTimestampNs::new(1_009_400_000);
        let interval = Duration::from_nanos(6_060_606);
        let mut deadline = PresentationDeadlinePlanner::new(interval);
        deadline.note_presented(anchor);

        let reservation = plan_composited_variable_reservation(
            prepared_adaptive_sync(Some(adaptive_sync_validation_key()), true, true, false),
            reactive_double_admission(),
            &deadline,
            Some(anchor),
            now,
            interval,
        )
        .expect("qualified steady-state AdaptiveSync is variable");
        let PresentationTiming::VrrWindow {
            earliest_present,
            preferred_present,
            presentation_not_before,
            ..
        } = reservation.timing()
        else {
            panic!("runtime reservation must retain VRR timing");
        };
        assert_eq!(earliest_present.get(), 1_006_060_606);
        assert_eq!(preferred_present.get(), 1_009_400_000);
        assert_eq!(presentation_not_before, None);
        assert_ne!(preferred_present.get(), 1_012_121_212);
        assert_eq!(
            reservation.opportunity().id(),
            oblivion_one::native::buffering::PresentationOpportunityId::new(1, 2)
        );

        let prepared = PreparedCompositedReservation::Variable { reservation };
        assert_eq!(
            prepared.presentation_timing(MonotonicTimestampNs::new(1_010_000_000)),
            KmsPresentationTiming::Variable {
                earliest_submit_ns: 1_010_000_000,
            }
        );
        assert_eq!(prepared.fixed_target(), None);
        assert_eq!(prepared.fixed_submit_window(), None);
    }

    #[test]
    fn variable_admission_requires_exact_accepted_confirmed_adaptive_sync() {
        let anchor = MonotonicTimestampNs::new(10_000_000);
        let now = MonotonicTimestampNs::new(12_000_000);
        let interval = Duration::from_nanos(6_060_606);
        let deadline = PresentationDeadlinePlanner::new(interval);
        let key = adaptive_sync_validation_key();

        for state in [
            prepared_adaptive_sync(None, false, true, false),
            prepared_adaptive_sync(Some(key), false, true, false),
            prepared_adaptive_sync(Some(key), true, false, false),
            prepared_adaptive_sync(Some(key), true, true, true),
        ] {
            assert!(
                plan_composited_variable_reservation(
                    state,
                    reactive_double_admission(),
                    &deadline,
                    Some(anchor),
                    now,
                    interval,
                )
                .is_none()
            );
        }

        let mut async_state = prepared_adaptive_sync(Some(key), true, true, false);
        async_state.mode = OutputPresentationMode::AdaptiveAsync;
        assert!(
            plan_composited_variable_reservation(
                async_state,
                reactive_double_admission(),
                &deadline,
                Some(anchor),
                now,
                interval,
            )
            .is_none()
        );

        let mut excluded = [reactive_double_admission(); 6];
        excluded[0].reactive_double = false;
        excluded[1].desired_credit_one = false;
        excluded[2].render_ahead = true;
        excluded[3].fixed_target_owned = true;
        excluded[4].commit_timing_applies = true;
        excluded[5].deferred_o1 = true;
        for admission in excluded {
            assert!(
                plan_composited_variable_reservation(
                    prepared_adaptive_sync(Some(key), true, true, false),
                    admission,
                    &deadline,
                    Some(anchor),
                    now,
                    interval,
                )
                .is_none()
            );
        }
    }

    #[test]
    fn render_domain_is_destroyed_before_physical_pool_release() {
        let order = RefCell::new(Vec::new());
        teardown_render_before_physical(
            || order.borrow_mut().push("render"),
            || order.borrow_mut().push("physical"),
        );
        assert_eq!(order.into_inner(), ["render", "physical"]);
    }

    #[derive(Default)]
    struct FutureRenderAdmissionState {
        worker_busy: bool,
        render_demand: bool,
    }

    impl FutureRenderAdmissionState {
        fn dispatch(&mut self) {
            assert!(!self.worker_busy);
            self.worker_busy = true;
            self.render_demand = false;
        }

        fn visual_change(&mut self) {
            if self.worker_busy {
                self.render_demand = true;
            }
        }

        fn worker_completed(&mut self) -> bool {
            self.worker_busy = false;
            if self.render_demand {
                self.dispatch();
                true
            } else {
                false
            }
        }
    }

    #[test]
    fn future_busy_worker_coalesces_many_visual_changes_to_one_newest_admission() {
        let mut state = FutureRenderAdmissionState::default();
        state.dispatch();

        for _ in 0..4 {
            state.visual_change();
        }
        assert!(state.worker_busy);
        assert!(state.render_demand);

        assert!(state.worker_completed());
        assert!(state.worker_busy);
        assert!(!state.render_demand);
        assert!(!state.worker_completed());
    }

    #[test]
    fn empty_composited_damage_is_forced_for_unrepresented_kms_state() {
        let vsync = OutputPresentationStateKey {
            mode: OutputPresentationMode::Vsync,
            content_type: DrmContentType::Graphics,
            output_generation: 7,
        };
        let adaptive = OutputPresentationStateKey {
            mode: OutputPresentationMode::AdaptiveSync,
            ..vsync
        };
        let transitions = [
            (vsync, adaptive),
            (adaptive, vsync),
            (
                vsync,
                OutputPresentationStateKey {
                    content_type: DrmContentType::Game,
                    ..vsync
                },
            ),
        ];

        for (confirmed, desired) in transitions {
            let transition_required =
                !kms_presentation_state_is_represented(desired, confirmed, None);
            assert!(transition_required);
            let damage = damage_for_physical_presentation_state(
                &crate::native_output::NativeOutputDamage::empty(),
                transition_required,
                1920,
                1080,
            );
            assert_eq!(
                damage.kind,
                crate::native_output::NativeDamageKind::FullOutput
            );
            assert_eq!(damage.pixels, 1920 * 1080);
        }
    }

    #[test]
    fn empty_composited_damage_stays_suppressed_when_state_is_confirmed_or_pending() {
        let confirmed = OutputPresentationStateKey {
            mode: OutputPresentationMode::Vsync,
            content_type: DrmContentType::Graphics,
            output_generation: 7,
        };
        let adaptive = OutputPresentationStateKey {
            mode: OutputPresentationMode::AdaptiveAsync,
            ..confirmed
        };

        for pending in [None, Some(confirmed), Some(adaptive)] {
            let desired = pending.unwrap_or(confirmed);
            assert!(kms_presentation_state_is_represented(
                desired, confirmed, pending
            ));
            let damage = damage_for_physical_presentation_state(
                &crate::native_output::NativeOutputDamage::empty(),
                false,
                1920,
                1080,
            );
            assert!(damage.is_empty());
        }
    }

    #[test]
    fn direct_scanout_capability_filter_keeps_exact_opaque_rgb_pairs() {
        let xrgb = DrmFormatModifierPair {
            fourcc: DrmFormat::XRGB8888_FOURCC,
            modifier: 7,
        };
        let xbgr = DrmFormatModifierPair {
            fourcc: DrmFormat::XBGR8888_FOURCC,
            modifier: 9,
        };
        let argb = DrmFormatModifierPair {
            fourcc: DrmFormat::ARGB8888_FOURCC,
            modifier: 11,
        };
        let invalid = DrmFormatModifierPair {
            fourcc: DrmFormat::XBGR8888_FOURCC,
            modifier: DrmModifier::INVALID.0,
        };
        let feedback = EglGlesDmabufFeedback::with_scanout_tranche(
            [],
            [
                EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier(7)),
                EglGlesDmabufFormat::new(DrmFormat::Xbgr8888, DrmModifier(9)),
                EglGlesDmabufFormat::new(DrmFormat::Argb8888, DrmModifier(11)),
            ],
        );
        let capabilities =
            discover_direct_scanout_capabilities(&[xrgb, xbgr, argb, invalid], &feedback);

        assert_eq!(
            capabilities,
            vec![
                DirectScanoutFormatCapability {
                    format: xrgb.fourcc,
                    modifier: xrgb.modifier,
                },
                DirectScanoutFormatCapability {
                    format: xbgr.fourcc,
                    modifier: xbgr.modifier,
                },
            ]
        );
    }

    #[test]
    fn direct_scanout_capability_filter_uses_plane_and_renderer_intersection_only() {
        let xrgb = DrmFormatModifierPair {
            fourcc: DrmFormat::XRGB8888_FOURCC,
            modifier: 7,
        };
        let xbgr = DrmFormatModifierPair {
            fourcc: DrmFormat::XBGR8888_FOURCC,
            modifier: 9,
        };
        let feedback = EglGlesDmabufFeedback::with_scanout_tranche(
            [],
            [EglGlesDmabufFormat::new(
                DrmFormat::Xrgb8888,
                DrmModifier(7),
            )],
        );
        let capabilities = discover_direct_scanout_capabilities(&[xrgb, xbgr], &feedback);

        assert_eq!(
            capabilities,
            vec![DirectScanoutFormatCapability {
                format: xrgb.fourcc,
                modifier: xrgb.modifier,
            }]
        );
    }

    #[test]
    fn kms_presentable_client_formats_are_broader_than_direct_scanout_capabilities() {
        let argb = DrmFormatModifierPair {
            fourcc: DrmFormat::ARGB8888_FOURCC,
            modifier: 11,
        };
        let feedback = EglGlesDmabufFeedback::with_scanout_tranche(
            [],
            [EglGlesDmabufFormat::new(
                DrmFormat::Argb8888,
                DrmModifier(11),
            )],
        );

        let kms_presentable = discover_kms_presentable_client_formats(&[argb], feedback.formats());
        let direct = discover_direct_scanout_capabilities(&[argb], &feedback);

        assert_eq!(kms_presentable, vec![argb]);
        assert!(direct.is_empty());
    }

    #[test]
    fn kms_preferred_feedback_uses_selected_renderer_set() {
        let safe = EglGlesDmabufFormat::new(DrmFormat::Xbgr8888, DrmModifier(2));
        let unsafe_modifier = EglGlesDmabufFormat::new(DrmFormat::Xbgr8888, DrmModifier(3));
        let selection = resolve_kms_preferred_dmabuf_policy(
            KmsPreferredDmabufPolicy::Force,
            "test vendor",
            &[safe, unsafe_modifier],
            &[DrmFormatModifierPair {
                fourcc: DrmFormat::XBGR8888_FOURCC,
                modifier: 2,
            }],
        );
        let feedback = EglGlesDmabufFeedback::with_scanout_tranche(
            [],
            selection.advertised_renderer_formats.clone(),
        );

        assert_eq!(feedback.formats(), &[safe]);
        assert_eq!(feedback.format_table_formats(), &[safe]);
        assert!(!feedback.advertises(DrmFormat::Xbgr8888, DrmModifier(3)));
    }
}
