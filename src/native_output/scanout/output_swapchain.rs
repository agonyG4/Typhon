use std::{
    io,
    os::fd::{AsRawFd, OwnedFd, RawFd},
};

#[cfg(test)]
use std::num::NonZeroU64;

use oblivion_one::compositor::{CompositorFrameBatchId, SurfaceDamagePresentation};
use oblivion_one::native::adaptive_buffering::FenceTimestampQuality;
use oblivion_one::native::buffering::O1AdmissionObservation;
use oblivion_one::native::kms::{FramebufferId, PageFlipToken};
#[cfg(test)]
use oblivion_one::native::presentation_deadline::PresentationTargetReason;
use oblivion_one::native::presentation_deadline::{
    MonotonicTimestampNs, PresentationTarget, PrimaryRefreshClaim,
};
use oblivion_one::native::scheduler::NativeOutputPacingMode;

use crate::egl_renderer::{EglSceneFrameCommit, native_fence::NativeRenderFence};
use crate::native_output::OutputTransactionId;
use crate::native_output::output::{CursorFramebufferPin, NativeCursorImageKey};
use crate::native_output::presentation::plane::CursorRevision;
#[cfg(test)]
use crate::native_output::presentation::transaction::O1PrepareIntent;
use crate::native_output::presentation::transaction::{
    FramePresentationReservation, O1PredecessorAnchor,
};
use crate::native_output::presentation::{
    kms_timing::KmsSubmitWindow,
    plane::{CursorRevealTraceSnapshot, FrozenPrimaryCursorPlan},
    plane_policy::CursorCapabilityKey,
};
use oblivion_one::native::buffering::PresentationOpportunityFrontier;

pub(crate) const EXPLICIT_OUTPUT_SLOT_CAPACITY: usize = 3;
const SUSPENDED_OUTPUT_SLOT_CAPACITY: usize = EXPLICIT_OUTPUT_SLOT_CAPACITY - 1;

pub(crate) const fn output_slot_linear_payload_bytes(width: u32, height: u32) -> u64 {
    (width as u64)
        .saturating_mul(height as u64)
        .saturating_mul(4)
}

pub(crate) const fn output_pool_linear_payload_bytes(width: u32, height: u32) -> u64 {
    output_slot_linear_payload_bytes(width, height)
        .saturating_mul(EXPLICIT_OUTPUT_SLOT_CAPACITY as u64)
}

pub(crate) const fn incremental_third_slot_linear_payload_bytes(width: u32, height: u32) -> u64 {
    output_slot_linear_payload_bytes(width, height)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct OutputSlotId(u8);

impl OutputSlotId {
    pub(crate) const fn new(value: u8) -> Option<Self> {
        if value < EXPLICIT_OUTPUT_SLOT_CAPACITY as u8 {
            Some(Self(value))
        } else {
            None
        }
    }

    pub(crate) const fn get(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputSlotSet {
    slots: [OutputSlotId; EXPLICIT_OUTPUT_SLOT_CAPACITY],
}

impl OutputSlotSet {
    pub(crate) fn new(slots: [OutputSlotId; EXPLICIT_OUTPUT_SLOT_CAPACITY]) -> io::Result<Self> {
        if slots[0] == slots[1] || slots[0] == slots[2] || slots[1] == slots[2] {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "explicit output pool slot IDs must be unique",
            ));
        }
        Ok(Self { slots })
    }

    pub(crate) const fn capacity(self) -> usize {
        self.slots.len()
    }

    const fn contains(self, slot: OutputSlotId) -> bool {
        self.slots[0].0 == slot.0 || self.slots[1].0 == slot.0 || self.slots[2].0 == slot.0
    }

    fn iter(self) -> impl Iterator<Item = OutputSlotId> {
        self.slots.into_iter()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputSlotOwnership {
    slots: OutputSlotSet,
    current: OutputSlotId,
    pending: Option<OutputSlotId>,
    ready: Option<OutputSlotId>,
}

impl OutputSlotOwnership {
    pub(crate) fn from_presented_slots(
        slots: OutputSlotSet,
        current: Option<OutputSlotId>,
    ) -> io::Result<Self> {
        let current = current.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "explicit output swapchain requires a presented current slot",
            )
        })?;
        if !slots.contains(current) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "presented current slot does not belong to the explicit output pool",
            ));
        }
        Ok(Self {
            slots,
            current,
            pending: None,
            ready: None,
        })
    }

    pub(crate) fn set_pending(&mut self, slot: OutputSlotId) -> io::Result<()> {
        if self.pending.is_some() {
            return Err(io::Error::other("an output pageflip is already pending"));
        }
        self.ensure_free(slot)?;
        self.pending = Some(slot);
        Ok(())
    }

    pub(crate) fn set_ready(&mut self, slot: OutputSlotId) -> io::Result<()> {
        if self.ready.is_some() {
            return Err(io::Error::other("an output frame is already ready"));
        }
        self.ensure_free(slot)?;
        self.ready = Some(slot);
        Ok(())
    }

    fn ensure_free(&self, slot: OutputSlotId) -> io::Result<()> {
        if !self.slots.contains(slot) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "output slot does not belong to the explicit output pool",
            ));
        }
        if slot == self.current || self.pending == Some(slot) || self.ready == Some(slot) {
            return Err(io::Error::other("output slot is already owned"));
        }
        Ok(())
    }
}

/// Fatal output ownership failures. Recoverable session ownership lives in `suspended`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum OutputQuarantineReason {
    PostDrawRenderFailure,
    RenderFenceExportFailure,
    AtomicSubmitFailure,
}

#[derive(Debug)]
pub(crate) struct QuarantinedOutputSlot {
    pub(crate) slot: OutputSlotId,
    pub(crate) pool_generation: u64,
    pub(crate) timing_fence: Option<OwnedFd>,
    pub(crate) reason: OutputQuarantineReason,
    abandoned_frame: Option<RenderedOutputFrame>,
}

#[derive(Debug)]
struct SuspendedOutputSlot {
    slot: OutputSlotId,
    pool_generation: u64,
    timing_fence: Option<OwnedFd>,
    abandoned_frame: Option<RenderedOutputFrame>,
}

#[derive(Debug)]
pub(crate) struct RenderedOutputFrame {
    pub(crate) id: u64,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) slot: OutputSlotId,
    pub(crate) framebuffer_id: FramebufferId,
    pub(crate) render_generation: u64,
    pub(crate) pool_generation: u64,
    pub(crate) reservation: FramePresentationReservation,
    pub(crate) submit_window: KmsSubmitWindow,
    pub(crate) render_fence: NativeRenderFence,
    pub(crate) fence_timing_evidence: Option<RenderFenceTimingEvidence>,
    pub(crate) scene_commit: EglSceneFrameCommit,
    pub(crate) surface_damage: SurfaceDamagePresentation,
    pub(crate) protocol_batch_id: CompositorFrameBatchId,
    pub(crate) composite_started_at: MonotonicTimestampNs,
    pub(crate) fence_exported_at: MonotonicTimestampNs,
    pub(crate) rendered_at: MonotonicTimestampNs,
    pub(crate) client_commit_ns: Option<u64>,
    pub(crate) callback_reaction_ns: Option<u64>,
    pub(crate) callback_admission_ns: Option<u64>,
    pub(crate) callback_surface_id: Option<u32>,
    pub(crate) hardware_cursor_surface_id: Option<u32>,
    pub(crate) cpu_prepass_duration_ns: u64,
    pub(crate) cpu_encode_duration_ns: u64,
    pub(crate) frozen_cursor_plan: FrozenPrimaryCursorPlan,
    pub(crate) frozen_cursor_plane_owner: Option<FrozenCursorPlaneOwner>,
    pub(crate) frozen_cursor_trace_reveal: Option<CursorRevealTraceSnapshot>,
    pub(crate) o1_admission: Option<O1AdmissionObservation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RenderFenceTimingEvidence {
    pub(crate) signaled_at: MonotonicTimestampNs,
    pub(crate) quality: FenceTimestampQuality,
    pub(crate) render_sample_recorded: bool,
}

impl RenderedOutputFrame {
    pub(crate) const fn bound_target(&self) -> Option<PresentationTarget> {
        self.reservation.bound_target()
    }

    pub(crate) const fn is_deferred_o1(&self) -> bool {
        self.reservation.is_deferred_o1()
    }

    pub(crate) fn sample_fence_timing(
        &mut self,
        observed_at: MonotonicTimestampNs,
    ) -> io::Result<Option<RenderFenceTimingEvidence>> {
        if let Some(evidence) = self.fence_timing_evidence {
            return Ok(Some(evidence));
        }
        let Some((signaled_at, quality)) = self
            .render_fence
            .sample_timing_nonblocking(observed_at.get())?
        else {
            return Ok(None);
        };
        let evidence = RenderFenceTimingEvidence {
            signaled_at: MonotonicTimestampNs::new(signaled_at),
            quality,
            render_sample_recorded: false,
        };
        self.fence_timing_evidence = Some(evidence);
        Ok(Some(evidence))
    }

    pub(crate) fn mark_fence_timing_accounted(&mut self) {
        if let Some(evidence) = &mut self.fence_timing_evidence {
            evidence.render_sample_recorded = true;
        }
    }
}

#[derive(Debug)]
pub(crate) struct FrozenCursorPlaneOwner {
    pub(crate) revision: CursorRevision,
    pub(crate) client_source_key: Option<NativeCursorImageKey>,
    pub(crate) capability_key: Option<CursorCapabilityKey>,
    pub(crate) pin: Option<CursorFramebufferPin>,
}

#[derive(Debug)]
pub(crate) struct SubmittedOutputFrame {
    pub(crate) frame: RenderedOutputFrame,
    pub(crate) token: PageFlipToken,
    pub(crate) submit_started_at: MonotonicTimestampNs,
    pub(crate) submit_returned_at: MonotonicTimestampNs,
    pub(crate) out_fence: Option<OwnedFd>,
}

#[derive(Debug)]
pub(crate) struct WorkerQueuedOutputFrame {
    pub(crate) frame: RenderedOutputFrame,
    pub(crate) token: PageFlipToken,
    pub(crate) queued_at: MonotonicTimestampNs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputFrameIdentitySnapshot {
    pub(crate) frame_id: u64,
    pub(crate) protocol_batch_id: CompositorFrameBatchId,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) slot: OutputSlotId,
    pub(crate) framebuffer_id: FramebufferId,
    pub(crate) render_generation: u64,
    pub(crate) pool_generation: u64,
    pub(crate) target: Option<PresentationTarget>,
}

/// Immutable identity of one physical output frame.
///
/// Presentation state, including the deferred O1 target, is deliberately
/// excluded: it may change while the same physical frame remains owned by
/// the output pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct OutputFrameKey {
    pub(crate) frame_id: u64,
    pub(crate) protocol_batch_id: CompositorFrameBatchId,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) slot: OutputSlotId,
    pub(crate) framebuffer_id: FramebufferId,
    pub(crate) render_generation: u64,
    pub(crate) pool_generation: u64,
}

impl From<&OutputFrameIdentitySnapshot> for OutputFrameKey {
    fn from(snapshot: &OutputFrameIdentitySnapshot) -> Self {
        Self {
            frame_id: snapshot.frame_id,
            protocol_batch_id: snapshot.protocol_batch_id,
            transaction_id: snapshot.transaction_id,
            slot: snapshot.slot,
            framebuffer_id: snapshot.framebuffer_id,
            render_generation: snapshot.render_generation,
            pool_generation: snapshot.pool_generation,
        }
    }
}

impl From<&RenderedOutputFrame> for OutputFrameKey {
    fn from(frame: &RenderedOutputFrame) -> Self {
        Self {
            frame_id: frame.id,
            protocol_batch_id: frame.protocol_batch_id,
            transaction_id: frame.transaction_id,
            slot: frame.slot,
            framebuffer_id: frame.framebuffer_id,
            render_generation: frame.render_generation,
            pool_generation: frame.pool_generation,
        }
    }
}

impl From<&RenderedOutputFrame> for OutputFrameIdentitySnapshot {
    fn from(frame: &RenderedOutputFrame) -> Self {
        Self {
            frame_id: frame.id,
            protocol_batch_id: frame.protocol_batch_id,
            transaction_id: frame.transaction_id,
            slot: frame.slot,
            framebuffer_id: frame.framebuffer_id,
            render_generation: frame.render_generation,
            pool_generation: frame.pool_generation,
            target: frame.bound_target(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueuedOutputFrameIdentitySnapshot {
    pub(crate) frame: OutputFrameIdentitySnapshot,
    pub(crate) token: PageFlipToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PhysicalPrimaryClaimViolation {
    GenerationMismatch,
    Regression,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PhysicalPrimaryClaimRevalidation {
    Valid,
    OvertakesReady {
        owner: OutputFrameIdentitySnapshot,
    },
    OvertakesWorkerQueued {
        owner: QueuedOutputFrameIdentitySnapshot,
    },
    Fatal(PhysicalPrimaryClaimViolation),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeferredO1BindingFailure {
    IdentityMismatch,
    GenerationMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeferredO1BindingReadiness {
    NotDeferred,
    WaitingForPredecessor,
    Bindable {
        predecessor: O1PredecessorAnchor,
        actual_claim: PrimaryRefreshClaim,
    },
    Stale(DeferredO1BindingFailure),
}

#[derive(Debug)]
pub(crate) struct CompletedOutputFrame {
    pub(crate) frame: RenderedOutputFrame,
    pub(crate) submit_started_at: MonotonicTimestampNs,
    pub(crate) submit_returned_at: MonotonicTimestampNs,
    pub(crate) old_current: OutputSlotId,
    pub(crate) new_current: OutputSlotId,
    pub(crate) presentation_serial: u64,
}

#[derive(Debug)]
pub(crate) struct AtomicOutputSwapchain {
    slots: OutputSlotSet,
    pool_generation: u64,
    current: OutputSlotId,
    worker_queued: Option<WorkerQueuedOutputFrame>,
    pending: Option<SubmittedOutputFrame>,
    ready: Option<RenderedOutputFrame>,
    rendering: Option<OutputSlotId>,
    quarantine: Option<QuarantinedOutputSlot>,
    suspended: [Option<SuspendedOutputSlot>; SUSPENDED_OUTPUT_SLOT_CAPACITY],
    next_frame_id: u64,
    presentation_serial: u64,
    current_framebuffer_id: Option<FramebufferId>,
    last_presented_primary_claim: Option<PrimaryRefreshClaim>,
    last_presented_primary_anchor: Option<(O1PredecessorAnchor, PrimaryRefreshClaim)>,
    last_completed_primary_anchor: Option<O1PredecessorAnchor>,
}

impl AtomicOutputSwapchain {
    pub(crate) fn from_presented_slots(
        slots: OutputSlotSet,
        current: OutputSlotId,
        pool_generation: u64,
    ) -> io::Result<Self> {
        OutputSlotOwnership::from_presented_slots(slots, Some(current))?;
        Ok(Self {
            slots,
            pool_generation,
            current,
            worker_queued: None,
            pending: None,
            ready: None,
            rendering: None,
            quarantine: None,
            suspended: std::array::from_fn(|_| None),
            next_frame_id: 1,
            presentation_serial: 0,
            current_framebuffer_id: None,
            last_presented_primary_claim: None,
            last_presented_primary_anchor: None,
            last_completed_primary_anchor: None,
        })
    }

    pub(crate) fn acquire_render_slot(&mut self) -> io::Result<OutputSlotId> {
        self.acquire_render_slot_for(NativeOutputPacingMode::PredictiveTriple)
    }

    pub(crate) fn acquire_render_slot_for(
        &mut self,
        pacing_mode: NativeOutputPacingMode,
    ) -> io::Result<OutputSlotId> {
        self.acquire_render_slot_for_limit(
            u8::from(pacing_mode == NativeOutputPacingMode::PredictiveTriple) + 1,
        )
    }

    pub(crate) fn acquire_render_slot_for_limit(
        &mut self,
        future_primary_limit: u8,
    ) -> io::Result<OutputSlotId> {
        self.ensure_operational()?;
        if self.rendering.is_some() {
            return Err(io::Error::other("an output slot is already rendering"));
        }
        if self.ready.is_some() {
            return Err(io::Error::other("an output frame is already ready"));
        }
        if future_primary_limit < 2 && self.pending.is_some() {
            return Err(io::Error::other(
                "future-primary limit cannot acquire a third output slot while pageflip is pending",
            ));
        }
        let slot = self
            .slots
            .iter()
            .find(|slot| self.slot_is_free(*slot))
            .ok_or_else(|| io::Error::other("no explicit output slot is free"))?;
        self.rendering = Some(slot);
        Ok(slot)
    }

    pub(crate) fn render_target_available_for(&self, pacing_mode: NativeOutputPacingMode) -> bool {
        self.render_target_available_for_limit(
            u8::from(pacing_mode == NativeOutputPacingMode::PredictiveTriple) + 1,
        )
    }

    pub(crate) fn render_target_available_for_limit(&self, future_primary_limit: u8) -> bool {
        self.quarantine.is_none()
            && !self.has_suspended_ownership()
            && self.rendering.is_none()
            && self.ready.is_none()
            && !(future_primary_limit < 2 && self.pending.is_some())
            && self.free_slot_count() > 0
    }

    pub(crate) const fn next_frame_id(&self) -> u64 {
        self.next_frame_id
    }

    pub(crate) fn advance_external_frame_id(&mut self, frame_id: u64) -> io::Result<()> {
        if frame_id != self.next_frame_id {
            return Err(io::Error::other(
                "external frame identity does not match the output sequence",
            ));
        }
        self.next_frame_id = self
            .next_frame_id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("output frame ID overflow"))?;
        Ok(())
    }

    pub(crate) const fn pool_generation(&self) -> u64 {
        self.pool_generation
    }

    pub(crate) const fn slot_capacity(&self) -> usize {
        self.slots.capacity()
    }

    pub(crate) fn cancel_render_before_gpu(&mut self, slot: OutputSlotId) -> io::Result<()> {
        if self.rendering != Some(slot) {
            return Err(io::Error::other(
                "cancelled output slot does not match active rendering ownership",
            ));
        }
        self.rendering = None;
        Ok(())
    }

    /// Release a rendering slot after GPU completion has been proven for an
    /// unpresented render. This must not be used before the renderer's work is
    /// complete: unlike `cancel_render_before_gpu`, GLES may have sampled
    /// client or scanout images before the render was abandoned.
    pub(crate) fn complete_unpresented_render(&mut self, slot: OutputSlotId) -> io::Result<()> {
        if self.rendering != Some(slot) {
            return Err(io::Error::other(
                "completed unpresented output slot does not match active rendering ownership",
            ));
        }
        // This transition creates no KMS ownership, advances no frame or
        // presentation serial, changes no buffer-age history, and leaves
        // ready/pending/worker ownership and fatal quarantine untouched.
        self.rendering = None;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn finish_render(
        &mut self,
        slot: OutputSlotId,
        render_generation: u64,
        render_fence: NativeRenderFence,
    ) -> io::Result<u64> {
        let now = MonotonicTimestampNs::new(self.next_frame_id);
        let target = PresentationTarget {
            sequence: self.next_frame_id,
            presentation_time: now,
            submit_not_before: now,
            render_start_deadline: now,
            refresh_interval: std::time::Duration::from_nanos(1),
            reason: PresentationTargetReason::ForcedValidation,
            clock_generation: self.pool_generation,
            estimated: true,
            predicted_unreachable: false,
            physical_claim: oblivion_one::native::presentation_deadline::PrimaryRefreshClaim {
                sequence: self.next_frame_id,
                presentation_time: now,
                clock_generation: self.pool_generation,
            },
            selection_evidence: Default::default(),
        };
        static NEXT_TEST_SERVER: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(1);
        let socket = format!(
            "typhon-output-swapchain-test-{}-{}",
            std::process::id(),
            NEXT_TEST_SERVER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let mut server = oblivion_one::compositor::OwnCompositorServer::bind(socket)
            .expect("test frame ownership server should bind");
        let protocol_batch_id = server.take_frame_batch_for_render(self.next_frame_id);
        self.finish_render_owned(RenderedOutputFrame {
            id: self.next_frame_id,
            transaction_id: OutputTransactionId::new(
                NonZeroU64::new(self.next_frame_id).expect("test transaction ID is nonzero"),
            ),
            slot,
            framebuffer_id: FramebufferId::new(42).expect("test framebuffer ID is nonzero"),
            render_generation,
            pool_generation: self.pool_generation,
            reservation: FramePresentationReservation::Bound(target),
            submit_window: KmsSubmitWindow::try_new(
                target.presentation_time.get(),
                target.submit_not_before().get(),
                0,
                0,
            )
            .expect("test output frame has a reachable submit window"),
            render_fence,
            fence_timing_evidence: None,
            scene_commit: EglSceneFrameCommit::empty_for_test(),
            surface_damage: SurfaceDamagePresentation::default(),
            protocol_batch_id,
            composite_started_at: now,
            fence_exported_at: now,
            rendered_at: now,
            client_commit_ns: None,
            callback_reaction_ns: None,
            callback_admission_ns: None,
            callback_surface_id: None,
            hardware_cursor_surface_id: None,
            cpu_prepass_duration_ns: 0,
            cpu_encode_duration_ns: 0,
            frozen_cursor_plan: FrozenPrimaryCursorPlan {
                delivery: crate::native_output::presentation::plane::PresentedCursorDelivery::Hidden,
                primary_presentation:
                    crate::native_output::presentation::plane::FrozenPrimaryCursorPresentation::Preserve,
                cursor_test_policy:
                    crate::native_output::presentation::plane::FrozenCursorTestPolicy::Skip,
            },
            frozen_cursor_plane_owner: None,
            frozen_cursor_trace_reveal: None,
            o1_admission: None,
        })
    }

    pub(crate) fn finish_render_owned(&mut self, frame: RenderedOutputFrame) -> io::Result<u64> {
        self.ensure_operational()?;
        if self.rendering != Some(frame.slot) {
            return Err(io::Error::other(
                "finished output slot does not match active rendering ownership",
            ));
        }
        if self.ready.is_some() {
            return Err(io::Error::other("an output frame is already ready"));
        }
        if frame.id != self.next_frame_id || frame.pool_generation != self.pool_generation {
            return Err(io::Error::other(
                "rendered output frame identity does not match the swapchain",
            ));
        }
        if let Some(target) = frame.bound_target() {
            self.validate_later_primary_target(target)?;
        } else if !frame.is_deferred_o1() {
            return Err(io::Error::other("rendered output frame has no reservation"));
        }
        let frame_id = frame.id;
        let next_frame_id = self
            .next_frame_id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("output frame ID overflow"))?;
        self.next_frame_id = next_frame_id;
        self.rendering = None;
        self.ready = Some(frame);
        Ok(frame_id)
    }

    pub(crate) fn ready_cursor_plan(&self) -> Option<FrozenPrimaryCursorPlan> {
        self.ready.as_ref().map(|frame| frame.frozen_cursor_plan)
    }

    pub(crate) fn ready_cursor_plane_owner(&self) -> Option<&FrozenCursorPlaneOwner> {
        self.ready
            .as_ref()
            .and_then(|frame| frame.frozen_cursor_plane_owner.as_ref())
    }

    pub(crate) fn ready_cursor_trace_reveal(&self) -> Option<CursorRevealTraceSnapshot> {
        self.ready
            .as_ref()
            .and_then(|frame| frame.frozen_cursor_trace_reveal)
    }

    #[cfg(test)]
    pub(crate) fn set_ready_cursor_trace_reveal_for_test(
        &mut self,
        snapshot: Option<CursorRevealTraceSnapshot>,
    ) -> io::Result<()> {
        self.ready
            .as_mut()
            .ok_or_else(|| io::Error::other("no rendered output frame is ready"))?
            .frozen_cursor_trace_reveal = snapshot;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn prepare_ready_for_test(
        &mut self,
        slot: OutputSlotId,
        render_fence: NativeRenderFence,
        frozen_cursor_plan: FrozenPrimaryCursorPlan,
        frozen_cursor_plane_owner: Option<FrozenCursorPlaneOwner>,
    ) -> io::Result<()> {
        self.ensure_operational()?;
        if self.rendering != Some(slot) || self.ready.is_some() {
            return Err(io::Error::other("test ready frame ownership mismatch"));
        }
        let now = MonotonicTimestampNs::new(self.next_frame_id);
        let frame_id = self.next_frame_id;
        self.next_frame_id = self
            .next_frame_id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("test output frame ID overflow"))?;
        self.rendering = None;
        self.ready = Some(RenderedOutputFrame {
            id: frame_id,
            transaction_id: OutputTransactionId::new(
                std::num::NonZeroU64::new(frame_id).expect("test transaction ID is nonzero"),
            ),
            slot,
            framebuffer_id: FramebufferId::new(42).expect("test framebuffer ID is nonzero"),
            render_generation: 1,
            pool_generation: self.pool_generation,
            reservation: FramePresentationReservation::Bound(PresentationTarget {
                sequence: frame_id,
                presentation_time: now,
                submit_not_before: now,
                render_start_deadline: now,
                refresh_interval: std::time::Duration::from_nanos(1),
                reason: PresentationTargetReason::ForcedValidation,
                clock_generation: self.pool_generation,
                estimated: true,
                predicted_unreachable: false,
                physical_claim: oblivion_one::native::presentation_deadline::PrimaryRefreshClaim {
                    sequence: frame_id,
                    presentation_time: now,
                    clock_generation: self.pool_generation,
                },
                selection_evidence: Default::default(),
            }),
            submit_window: KmsSubmitWindow::try_new(now.get(), now.get(), 0, 0)
                .expect("test ready frame has a reachable submit window"),
            render_fence,
            fence_timing_evidence: None,
            scene_commit: EglSceneFrameCommit::empty_for_test(),
            surface_damage: SurfaceDamagePresentation::default(),
            protocol_batch_id: CompositorFrameBatchId::new(
                std::num::NonZeroU64::new(frame_id).expect("test batch ID is nonzero"),
            ),
            composite_started_at: now,
            fence_exported_at: now,
            rendered_at: now,
            client_commit_ns: None,
            callback_reaction_ns: None,
            callback_admission_ns: None,
            callback_surface_id: None,
            hardware_cursor_surface_id: None,
            cpu_prepass_duration_ns: 0,
            cpu_encode_duration_ns: 0,
            frozen_cursor_plan,
            frozen_cursor_plane_owner,
            frozen_cursor_trace_reveal: None,
            o1_admission: None,
        });
        Ok(())
    }

    pub(crate) fn submit_ready(
        &mut self,
        token: PageFlipToken,
        out_fence: Option<OwnedFd>,
    ) -> io::Result<()> {
        self.submit_ready_timed(
            token,
            out_fence,
            MonotonicTimestampNs::new(0),
            MonotonicTimestampNs::new(0),
        )
    }

    pub(crate) fn submit_ready_timed(
        &mut self,
        token: PageFlipToken,
        out_fence: Option<OwnedFd>,
        submit_started_at: MonotonicTimestampNs,
        submit_returned_at: MonotonicTimestampNs,
    ) -> io::Result<()> {
        self.ensure_operational()?;
        if self.pending.is_some() {
            return Err(io::Error::other("an output pageflip is already pending"));
        }
        let frame = self.take_ready_for_submission()?;
        self.submission_succeeded(
            frame,
            token,
            out_fence,
            submit_started_at,
            submit_returned_at,
        )
    }

    pub(crate) fn take_ready_for_worker(
        &mut self,
        token: PageFlipToken,
        queued_at: MonotonicTimestampNs,
    ) -> io::Result<(OwnedFd, Option<FrozenCursorPlaneOwner>)> {
        self.ensure_operational()?;
        if self.worker_queued.is_some() {
            return Err(io::Error::other(
                "an output Atomic commit is already owned by the worker",
            ));
        }
        let ready = self
            .ready
            .as_ref()
            .ok_or_else(|| io::Error::other("no rendered output frame is ready"))?;
        self.validate_worker_queued_frame(ready)?;
        let mut frame = self
            .ready
            .take()
            .ok_or_else(|| io::Error::other("no rendered output frame is ready"))?;
        let fence = match frame.render_fence.take_submission_fd() {
            Ok(fence) => fence,
            Err(error) => {
                self.ready = Some(frame);
                return Err(error);
            }
        };
        self.worker_queued = Some(WorkerQueuedOutputFrame {
            frame,
            token,
            queued_at,
        });
        // The FD is returned separately so the caller can move it into the
        // cross-thread job while this holder retains all EGL/GBM ownership.
        let owner = self
            .worker_queued
            .as_mut()
            .and_then(|queued| queued.frame.frozen_cursor_plane_owner.take());
        Ok((fence, owner))
    }

    pub(crate) fn worker_queued_cursor_trace_reveal(&self) -> Option<CursorRevealTraceSnapshot> {
        self.worker_queued
            .as_ref()
            .and_then(|queued| queued.frame.frozen_cursor_trace_reveal)
    }

    pub(crate) fn store_worker_queued(
        &mut self,
        queued: WorkerQueuedOutputFrame,
    ) -> io::Result<()> {
        self.ensure_operational()?;
        if self.worker_queued.is_some() {
            return Err(io::Error::other(
                "an output Atomic commit is already owned by the worker",
            ));
        }
        self.validate_worker_queued_frame(&queued.frame)?;
        self.worker_queued = Some(queued);
        Ok(())
    }

    pub(crate) fn promote_worker_queued(
        &mut self,
        token: PageFlipToken,
        out_fence: Option<OwnedFd>,
        submit_started_at: MonotonicTimestampNs,
        submit_returned_at: MonotonicTimestampNs,
    ) -> io::Result<()> {
        self.ensure_operational()?;
        if self.pending.is_some() {
            return Err(io::Error::other(
                "worker success arrived while an output pageflip is already pending",
            ));
        }
        let queued = self
            .worker_queued
            .take()
            .ok_or_else(|| io::Error::other("worker success arrived without queued output"))?;
        if queued.token != token {
            self.worker_queued = Some(queued);
            return Err(io::Error::other(
                "worker success token mismatches queued output",
            ));
        }
        if queued.frame.pool_generation != self.pool_generation {
            self.worker_queued = Some(queued);
            return Err(io::Error::other(
                "worker success output frame belongs to an old pool generation",
            ));
        }
        self.pending = Some(SubmittedOutputFrame {
            frame: queued.frame,
            token,
            submit_started_at,
            submit_returned_at,
            out_fence,
        });
        Ok(())
    }

    pub(crate) fn return_worker_queued_for_replan(
        &mut self,
        token: PageFlipToken,
        submission_fence: &mut Option<OwnedFd>,
        cursor_owner: &mut Option<FrozenCursorPlaneOwner>,
    ) -> io::Result<bool> {
        if self.quarantine.is_some() {
            return Err(io::Error::other(
                "cannot re-plan an output while a slot is quarantined",
            ));
        }
        if self.ready.is_some() {
            return Err(io::Error::other(
                "cannot return worker output while another frame is ready",
            ));
        }
        let Some(queued) = self.worker_queued.as_ref() else {
            return Ok(false);
        };
        if queued.token != token {
            return Err(io::Error::other(
                "re-planned worker output token does not match queued ownership",
            ));
        }
        if queued.frame.pool_generation != self.pool_generation {
            return Err(io::Error::other(
                "re-planned worker output frame belongs to an old pool generation",
            ));
        }
        if queued.frame.frozen_cursor_plane_owner.is_some() {
            return Err(io::Error::other(
                "re-planned worker output frame already owns a frozen cursor",
            ));
        }
        if submission_fence.is_none() {
            return Err(io::Error::other(
                "re-planned worker output is missing its input fence",
            ));
        }
        let mut queued = self
            .worker_queued
            .take()
            .expect("worker was observed above");
        if let Err(error) = queued
            .frame
            .render_fence
            .restore_submission_fd(submission_fence)
        {
            self.worker_queued = Some(queued);
            return Err(error);
        }
        queued.frame.frozen_cursor_plane_owner = cursor_owner.take();
        self.ready = Some(queued.frame);
        Ok(true)
    }

    pub(crate) fn fail_worker_queued(
        &mut self,
        token: PageFlipToken,
    ) -> io::Result<RenderedOutputFrame> {
        let queued = self
            .worker_queued
            .take()
            .ok_or_else(|| io::Error::other("worker failure arrived without queued output"))?;
        if queued.token != token {
            self.worker_queued = Some(queued);
            return Err(io::Error::other(
                "worker failure token mismatches queued output",
            ));
        }
        let mut frame = queued.frame;
        let timing_fence = frame.render_fence.take_timing_fd();
        self.quarantine_slot(
            frame.slot,
            timing_fence,
            OutputQuarantineReason::AtomicSubmitFailure,
            None,
        )?;
        Ok(frame)
    }

    pub(crate) fn take_ready_for_submission(&mut self) -> io::Result<RenderedOutputFrame> {
        self.ensure_operational()?;
        if self.pending.is_some() || self.worker_queued.is_some() {
            return Err(io::Error::other(
                "an output Atomic commit is already owned by the worker or kernel",
            ));
        }
        let ready = self
            .ready
            .as_ref()
            .ok_or_else(|| io::Error::other("no rendered output frame is ready"))?;
        let target = ready
            .bound_target()
            .ok_or_else(|| io::Error::other("unbound output frame cannot be submitted"))?;
        self.validate_later_primary_target(target)?;
        self.ready
            .take()
            .ok_or_else(|| io::Error::other("no rendered output frame is ready"))
    }

    pub(crate) fn submission_succeeded(
        &mut self,
        frame: RenderedOutputFrame,
        token: PageFlipToken,
        out_fence: Option<OwnedFd>,
        submit_started_at: MonotonicTimestampNs,
        submit_returned_at: MonotonicTimestampNs,
    ) -> io::Result<()> {
        self.ensure_operational()?;
        if self.pending.is_some()
            || self.worker_queued.is_some()
            || frame.pool_generation != self.pool_generation
        {
            return Err(io::Error::other(
                "submitted output frame does not match available pending ownership",
            ));
        }
        let target = frame
            .bound_target()
            .ok_or_else(|| io::Error::other("unbound output frame cannot be submitted"))?;
        self.validate_later_primary_target(target)?;
        self.pending = Some(SubmittedOutputFrame {
            frame,
            token,
            submit_started_at,
            submit_returned_at,
            out_fence,
        });
        Ok(())
    }

    pub(crate) fn submission_failed(
        &mut self,
        mut frame: RenderedOutputFrame,
    ) -> io::Result<RenderedOutputFrame> {
        if self.quarantine.is_some() {
            return Err(io::Error::other("an output slot is already quarantined"));
        }
        let timing_fence = frame.render_fence.take_timing_fd();
        self.quarantine_slot(
            frame.slot,
            timing_fence,
            OutputQuarantineReason::AtomicSubmitFailure,
            None,
        )?;
        Ok(frame)
    }

    pub(crate) fn atomic_submit_failed(&mut self) -> io::Result<OutputSlotId> {
        if self.quarantine.is_some() {
            return Err(io::Error::other("an output slot is already quarantined"));
        }
        let mut frame = self
            .ready
            .take()
            .ok_or_else(|| io::Error::other("no ready frame exists for failed Atomic submit"))?;
        let timing_fence = frame.render_fence.take_timing_fd();
        let slot = frame.slot;
        self.quarantine_slot(
            slot,
            timing_fence,
            OutputQuarantineReason::AtomicSubmitFailure,
            None,
        )?;
        Ok(slot)
    }

    pub(crate) fn quarantine_rendering(
        &mut self,
        timing_fence: Option<OwnedFd>,
        reason: OutputQuarantineReason,
    ) -> io::Result<OutputSlotId> {
        if self.quarantine.is_some() {
            return Err(io::Error::other("an output slot is already quarantined"));
        }
        let slot = self
            .rendering
            .take()
            .ok_or_else(|| io::Error::other("no rendering slot exists to quarantine"))?;
        self.quarantine_slot(slot, timing_fence, reason, None)?;
        Ok(slot)
    }

    pub(crate) fn suspend_abandon_ready(&mut self) -> io::Result<bool> {
        self.ensure_suspendable()?;
        let Some(ready) = self.ready.as_ref() else {
            return Ok(false);
        };
        let slot = ready.slot;
        if self.suspended_slot_id(slot).is_some() {
            return Err(io::Error::other(
                "output slot is already owned by suspended output",
            ));
        }
        let suspended_index = self.empty_suspended_slot_index()?;
        let mut frame = self.ready.take().expect("ready was observed above");
        let timing_fence = frame.render_fence.take_timing_fd();
        self.suspend_owned_slot(suspended_index, slot, timing_fence, frame);
        Ok(true)
    }

    pub(crate) fn suspend_abandon_worker_queued(
        &mut self,
        token: PageFlipToken,
    ) -> io::Result<bool> {
        let mut no_completion_fence = None;
        self.suspend_abandon_worker_queued_with_completion_fence(token, &mut no_completion_fence)
    }

    pub(crate) fn suspend_abandon_worker_queued_with_completion_fence(
        &mut self,
        token: PageFlipToken,
        completion_fence: &mut Option<OwnedFd>,
    ) -> io::Result<bool> {
        self.ensure_suspendable()?;
        let Some(queued) = self.worker_queued.as_ref() else {
            return Ok(false);
        };
        if queued.token != token {
            return Err(io::Error::other(
                "suspended worker output token does not match queued ownership",
            ));
        }
        if self.suspended_slot_id(queued.frame.slot).is_some() {
            return Err(io::Error::other(
                "output slot is already owned by suspended output",
            ));
        }
        let suspended_index = self.empty_suspended_slot_index()?;
        let mut queued = self
            .worker_queued
            .take()
            .expect("worker was observed above");
        let timing_fence = queued.frame.render_fence.take_timing_fd();
        let timing_fence = if timing_fence.is_some() {
            let _ = completion_fence.take();
            timing_fence
        } else {
            completion_fence.take()
        };
        self.suspend_owned_slot(
            suspended_index,
            queued.frame.slot,
            timing_fence,
            queued.frame,
        );
        Ok(true)
    }

    pub(crate) fn restore_worker_queued_submission_fence(
        &mut self,
        token: PageFlipToken,
        submission_fence: &mut Option<OwnedFd>,
    ) -> io::Result<()> {
        let queued = self
            .worker_queued
            .as_mut()
            .ok_or_else(|| io::Error::other("worker fence returned without queued output"))?;
        if queued.token != token {
            return Err(io::Error::other(
                "returned worker fence token mismatches queued output",
            ));
        }
        queued
            .frame
            .render_fence
            .restore_submission_fd(submission_fence)
    }

    /// Move all rendered frames that cannot participate in the next session
    /// generation into bounded suspend ownership in one preflighted operation.
    pub(crate) fn suspend_abandon_future_frames(&mut self) -> io::Result<()> {
        self.ensure_suspendable()?;
        let required =
            usize::from(self.worker_queued.is_some()) + usize::from(self.ready.is_some());
        let available = self
            .suspended
            .iter()
            .filter(|entry| entry.is_none())
            .count();
        if available < required {
            return Err(io::Error::other(
                "suspended output ownership exceeds explicit pool capacity",
            ));
        }
        if let Some(token) = self.worker_queued_token() {
            self.suspend_abandon_worker_queued(token)?;
        }
        self.suspend_abandon_ready()?;
        Ok(())
    }

    pub(crate) fn suspended_fences_signaled(&self) -> io::Result<bool> {
        self.suspended
            .iter()
            .flatten()
            .try_fold(true, |all_signaled, suspended| {
                let signaled = self.suspended_slot_fence_signaled(suspended)?;
                Ok(all_signaled && signaled)
            })
    }

    pub(crate) fn pending_fence_signaled(&self) -> io::Result<bool> {
        let Some(pending) = self.pending.as_ref() else {
            return Ok(true);
        };
        if let Some(timing_fence) = pending.frame.render_fence.timing_fd() {
            return poll_completion_fd(
                timing_fence.as_raw_fd(),
                "pending output render fence reported poll failure",
            );
        }
        if let Some(out_fence) = pending.out_fence.as_ref() {
            return poll_completion_fd(
                out_fence.as_raw_fd(),
                "pending output KMS out-fence reported poll failure",
            );
        }
        Err(io::Error::other(
            "pending output frame has no completion proof",
        ))
    }

    pub(crate) fn pending_fence_fd(&self) -> io::Result<Option<RawFd>> {
        if self.pending_fence_signaled()? {
            return Ok(None);
        }
        Ok(self.pending.as_ref().and_then(|pending| {
            pending
                .frame
                .render_fence
                .timing_fd()
                .map(AsRawFd::as_raw_fd)
                .or_else(|| pending.out_fence.as_ref().map(AsRawFd::as_raw_fd))
        }))
    }

    pub(crate) fn suspended_fence_fd(&self) -> io::Result<Option<RawFd>> {
        for suspended in self.suspended.iter().flatten() {
            if !self.suspended_slot_fence_signaled(suspended)? {
                return Ok(suspended
                    .timing_fence
                    .as_ref()
                    .map(AsRawFd::as_raw_fd)
                    .or_else(|| {
                        suspended
                            .abandoned_frame
                            .as_ref()
                            .and_then(|frame| frame.render_fence.readiness_fd())
                            .map(AsRawFd::as_raw_fd)
                    }));
            }
        }
        self.pending_fence_fd()
    }

    pub(crate) fn has_suspended_frame(&self) -> bool {
        self.suspended
            .iter()
            .flatten()
            .any(|suspended| suspended.abandoned_frame.is_some())
    }

    pub(crate) fn retire_pending_after_recovery(&mut self) -> Option<RenderedOutputFrame> {
        self.pending.take().map(|submitted| submitted.frame)
    }

    pub(crate) fn take_suspended_frame(&mut self) -> Option<RenderedOutputFrame> {
        self.suspended
            .iter_mut()
            .flatten()
            .find_map(|suspended| suspended.abandoned_frame.take())
    }

    pub(crate) fn rebind_pool_generation(&mut self, pool_generation: u64) -> io::Result<()> {
        if self.worker_queued.is_some()
            || self.pending.is_some()
            || self.ready.is_some()
            || self.rendering.is_some()
            || self.quarantine.is_some()
            || self.has_suspended_ownership()
        {
            return Err(io::Error::other(
                "output pool generation cannot change while a non-current slot is owned",
            ));
        }
        self.pool_generation = pool_generation;
        self.last_presented_primary_claim = None;
        self.last_presented_primary_anchor = None;
        self.last_completed_primary_anchor = None;
        Ok(())
    }

    pub(crate) fn recover_suspended_slot(&mut self, fence_signaled: bool) -> io::Result<()> {
        if self.quarantine.is_some() {
            return Err(io::Error::other(
                "fatal output quarantine cannot recover to normal operation",
            ));
        }
        if !fence_signaled || !self.suspended_fences_signaled()? {
            return Err(io::Error::other(
                "suspended output slot render fence is not signaled",
            ));
        }
        if self
            .suspended
            .iter()
            .flatten()
            .any(|suspended| suspended.abandoned_frame.is_some())
        {
            return Err(io::Error::other(
                "suspended output frame release ownership has not been retired",
            ));
        }
        for suspended in &mut self.suspended {
            *suspended = None;
        }
        Ok(())
    }

    pub(crate) fn complete_pageflip(
        &mut self,
        token: PageFlipToken,
        pool_generation: u64,
    ) -> io::Result<CompletedOutputFrame> {
        if self.is_poisoned() {
            return Err(io::Error::other(
                "fatal output quarantine blocks pageflip completion",
            ));
        }
        if pool_generation != self.pool_generation {
            return Err(io::Error::other("stale output pool generation pageflip"));
        }
        let pending = self
            .pending
            .as_ref()
            .ok_or_else(|| io::Error::other("pageflip arrived without a pending output frame"))?;
        if pending.frame.pool_generation != pool_generation || pending.token != token {
            return Err(io::Error::other("mismatched output pageflip token"));
        }
        let pending = self.pending.take().expect("pending was checked above");
        let old_current = self.current;
        self.current = pending.frame.slot;
        self.current_framebuffer_id = Some(pending.frame.framebuffer_id);
        self.last_completed_primary_anchor = Some(O1PredecessorAnchor {
            frame_id: pending.frame.id,
            transaction_id: pending.frame.transaction_id,
            token: pending.token,
            pool_generation: pending.frame.pool_generation,
        });
        self.presentation_serial = self
            .presentation_serial
            .checked_add(1)
            .ok_or_else(|| io::Error::other("output presentation serial overflow"))?;
        Ok(CompletedOutputFrame {
            submit_started_at: pending.submit_started_at,
            submit_returned_at: pending.submit_returned_at,
            frame: pending.frame,
            old_current,
            new_current: self.current,
            presentation_serial: self.presentation_serial,
        })
    }

    /// Record the phase actually consumed by the primary pageflip.  The
    /// planned claim remains immutable on the frame; a late physical pageflip
    /// can therefore invalidate a successor without silently retargeting it.
    pub(crate) fn note_physical_primary_presentation(
        &mut self,
        claim: PrimaryRefreshClaim,
    ) -> io::Result<()> {
        match self.revalidate_physical_primary_presentation(claim) {
            PhysicalPrimaryClaimRevalidation::Valid => {
                self.last_presented_primary_claim = Some(claim);
                self.last_presented_primary_anchor = self
                    .last_completed_primary_anchor
                    .take()
                    .map(|anchor| (anchor, claim));
                Ok(())
            }
            PhysicalPrimaryClaimRevalidation::OvertakesReady { .. }
            | PhysicalPrimaryClaimRevalidation::OvertakesWorkerQueued { .. } => Err(
                io::Error::other("physical primary claim overtakes a future primary owner"),
            ),
            PhysicalPrimaryClaimRevalidation::Fatal(violation) => Err(io::Error::other(format!(
                "physical primary claim violation: {violation:?}"
            ))),
        }
    }

    pub(crate) fn revalidate_physical_primary_presentation(
        &self,
        claim: PrimaryRefreshClaim,
    ) -> PhysicalPrimaryClaimRevalidation {
        if claim.clock_generation != self.pool_generation {
            return PhysicalPrimaryClaimRevalidation::Fatal(
                PhysicalPrimaryClaimViolation::GenerationMismatch,
            );
        }
        if let Some(last_presented) = self.last_presented_primary_claim
            && !is_strictly_later_claim(last_presented, claim)
        {
            return PhysicalPrimaryClaimRevalidation::Fatal(
                PhysicalPrimaryClaimViolation::Regression,
            );
        }
        if let Some(worker) = &self.worker_queued
            && worker
                .frame
                .bound_target()
                .is_some_and(|target| !is_strictly_later_claim(claim, target.physical_claim()))
        {
            return PhysicalPrimaryClaimRevalidation::OvertakesWorkerQueued {
                owner: QueuedOutputFrameIdentitySnapshot {
                    frame: (&worker.frame).into(),
                    token: worker.token,
                },
            };
        }
        if let Some(ready) = &self.ready
            && ready
                .bound_target()
                .is_some_and(|target| !is_strictly_later_claim(claim, target.physical_claim()))
        {
            return PhysicalPrimaryClaimRevalidation::OvertakesReady {
                owner: ready.into(),
            };
        }
        PhysicalPrimaryClaimRevalidation::Valid
    }

    pub(crate) const fn last_presented_primary_claim(&self) -> Option<PrimaryRefreshClaim> {
        self.last_presented_primary_claim
    }

    pub(crate) const fn last_presented_primary_anchor(
        &self,
    ) -> Option<(O1PredecessorAnchor, PrimaryRefreshClaim)> {
        self.last_presented_primary_anchor
    }

    pub(crate) const fn current(&self) -> OutputSlotId {
        self.current
    }

    pub(crate) const fn presentation_serial(&self) -> u64 {
        self.presentation_serial
    }

    pub(crate) const fn current_framebuffer_id(&self) -> Option<FramebufferId> {
        self.current_framebuffer_id
    }

    pub(crate) fn set_current_framebuffer_id(&mut self, framebuffer_id: FramebufferId) {
        self.current_framebuffer_id = Some(framebuffer_id);
    }

    pub(crate) fn pending_slot(&self) -> Option<OutputSlotId> {
        self.pending.as_ref().map(|pending| pending.frame.slot)
    }

    pub(crate) fn worker_queued_slot(&self) -> Option<OutputSlotId> {
        self.worker_queued.as_ref().map(|queued| queued.frame.slot)
    }

    pub(crate) fn worker_queued_token(&self) -> Option<PageFlipToken> {
        self.worker_queued.as_ref().map(|queued| queued.token)
    }

    pub(crate) fn pending_token(&self) -> Option<PageFlipToken> {
        self.pending.as_ref().map(|pending| pending.token)
    }

    pub(crate) fn pending_target(&self) -> Option<PresentationTarget> {
        self.pending
            .as_ref()
            .and_then(|pending| pending.frame.bound_target())
    }

    pub(crate) fn latest_future_primary_target(&self) -> Option<PresentationTarget> {
        self.worker_queued
            .as_ref()
            .and_then(|queued| queued.frame.bound_target())
            .or_else(|| self.pending_target())
    }

    pub(crate) fn deferred_o1_predecessor(&self) -> Option<O1PredecessorAnchor> {
        self.worker_queued
            .as_ref()
            .map(|queued| O1PredecessorAnchor {
                frame_id: queued.frame.id,
                transaction_id: queued.frame.transaction_id,
                token: queued.token,
                pool_generation: queued.frame.pool_generation,
            })
            .or_else(|| {
                self.pending.as_ref().map(|pending| O1PredecessorAnchor {
                    frame_id: pending.frame.id,
                    transaction_id: pending.frame.transaction_id,
                    token: pending.token,
                    pool_generation: pending.frame.pool_generation,
                })
            })
    }

    pub(crate) fn ready_is_deferred_o1(&self) -> bool {
        self.ready
            .as_ref()
            .is_some_and(RenderedOutputFrame::is_deferred_o1)
    }

    pub(crate) fn pending_identity(&self) -> Option<QueuedOutputFrameIdentitySnapshot> {
        self.pending
            .as_ref()
            .map(|pending| QueuedOutputFrameIdentitySnapshot {
                frame: (&pending.frame).into(),
                token: pending.token,
            })
    }

    pub(crate) fn worker_queued_identity(&self) -> Option<QueuedOutputFrameIdentitySnapshot> {
        self.worker_queued
            .as_ref()
            .map(|queued| QueuedOutputFrameIdentitySnapshot {
                frame: (&queued.frame).into(),
                token: queued.token,
            })
    }

    pub(crate) fn ready_identity(&self) -> Option<OutputFrameIdentitySnapshot> {
        self.ready.as_ref().map(Into::into)
    }

    pub(crate) fn deferred_o1_binding_failure(
        &self,
        output_generation: u64,
    ) -> Option<DeferredO1BindingFailure> {
        match self.deferred_o1_binding_readiness(output_generation) {
            DeferredO1BindingReadiness::Stale(failure) => Some(failure),
            DeferredO1BindingReadiness::NotDeferred
            | DeferredO1BindingReadiness::WaitingForPredecessor
            | DeferredO1BindingReadiness::Bindable { .. } => None,
        }
    }

    pub(crate) fn deferred_o1_binding_readiness(
        &self,
        output_generation: u64,
    ) -> DeferredO1BindingReadiness {
        // A historical last-presented mismatch is not stale while the exact
        // expected predecessor remains a live physical owner.
        let Some(ready) = self.ready.as_ref() else {
            return DeferredO1BindingReadiness::NotDeferred;
        };
        let FramePresentationReservation::DeferredO1(intent) = ready.reservation else {
            return DeferredO1BindingReadiness::NotDeferred;
        };
        if intent.output_generation != output_generation
            || intent.predecessor.pool_generation != self.pool_generation
            || intent.clock_generation != self.pool_generation
        {
            return DeferredO1BindingReadiness::Stale(DeferredO1BindingFailure::GenerationMismatch);
        }
        let live_predecessor = self.deferred_o1_predecessor();
        let Some((predecessor, actual_claim)) = self.last_presented_primary_anchor else {
            return if live_predecessor == Some(intent.predecessor) {
                DeferredO1BindingReadiness::WaitingForPredecessor
            } else {
                DeferredO1BindingReadiness::Stale(DeferredO1BindingFailure::IdentityMismatch)
            };
        };
        if intent.clock_generation != actual_claim.clock_generation {
            return DeferredO1BindingReadiness::Stale(DeferredO1BindingFailure::GenerationMismatch);
        }
        if intent.predecessor == predecessor {
            DeferredO1BindingReadiness::Bindable {
                predecessor,
                actual_claim,
            }
        } else if live_predecessor == Some(intent.predecessor) {
            DeferredO1BindingReadiness::WaitingForPredecessor
        } else {
            DeferredO1BindingReadiness::Stale(DeferredO1BindingFailure::IdentityMismatch)
        }
    }

    pub(crate) fn deferred_o1_binding_candidate(
        &self,
        output_generation: u64,
        bind_at: MonotonicTimestampNs,
    ) -> io::Result<
        Option<(
            OutputTransactionId,
            PresentationTarget,
            KmsSubmitWindow,
            u64,
        )>,
    > {
        let readiness = self.deferred_o1_binding_readiness(output_generation);
        self.deferred_o1_binding_candidate_for_readiness(readiness, bind_at)
    }

    pub(crate) fn deferred_o1_binding_candidate_for_readiness(
        &self,
        readiness: DeferredO1BindingReadiness,
        bind_at: MonotonicTimestampNs,
    ) -> io::Result<
        Option<(
            OutputTransactionId,
            PresentationTarget,
            KmsSubmitWindow,
            u64,
        )>,
    > {
        let (predecessor, actual_claim) = match readiness {
            DeferredO1BindingReadiness::NotDeferred
            | DeferredO1BindingReadiness::WaitingForPredecessor => return Ok(None),
            DeferredO1BindingReadiness::Stale(_) => {
                return Err(io::Error::other(
                    "deferred O1 predecessor identity is stale at binding",
                ));
            }
            DeferredO1BindingReadiness::Bindable {
                predecessor,
                actual_claim,
            } => (predecessor, actual_claim),
        };
        let Some(frame) = self.ready.as_ref() else {
            return Ok(None);
        };
        let FramePresentationReservation::DeferredO1(intent) = frame.reservation else {
            return Ok(None);
        };
        if intent.predecessor != predecessor {
            return Err(io::Error::other(
                "deferred O1 predecessor identity changed before binding",
            ));
        }
        let earliest_submit_ns = bind_at
            .get()
            .max(actual_claim.presentation_time.get().saturating_add(100_000));
        let earliest_submit = MonotonicTimestampNs::new(earliest_submit_ns);
        let mut claim = actual_claim
            .successor(intent.refresh_interval)
            .ok_or_else(|| io::Error::other("deferred O1 successor claim overflowed"))?;
        loop {
            let target = intent.bind_target(claim, earliest_submit);
            if let Ok(window) = frame
                .submit_window
                .rebind(target.presentation_time.get(), earliest_submit_ns)
            {
                let advanced_intervals = target
                    .physical_claim()
                    .sequence
                    .saturating_sub(actual_claim.sequence)
                    .saturating_sub(1);
                return Ok(Some((
                    frame.transaction_id,
                    target,
                    window,
                    advanced_intervals,
                )));
            }
            claim = claim
                .successor(intent.refresh_interval)
                .ok_or_else(|| io::Error::other("deferred O1 feasible claim overflowed"))?;
        }
    }

    pub(crate) fn commit_deferred_o1_binding(
        &mut self,
        transaction_id: OutputTransactionId,
        target: PresentationTarget,
        submit_window: KmsSubmitWindow,
    ) -> io::Result<()> {
        let ready_identity = self
            .ready
            .as_ref()
            .ok_or_else(|| io::Error::other("deferred O1 frame is no longer ready"))?;
        if ready_identity.transaction_id != transaction_id {
            return Err(io::Error::other(
                "deferred O1 transaction changed before binding",
            ));
        }
        if !ready_identity.is_deferred_o1() {
            return Err(io::Error::other("deferred O1 frame was already bound"));
        }
        self.validate_later_primary_target(target)?;
        let ready = self.ready.as_mut().expect("ready frame was observed above");
        ready.reservation = FramePresentationReservation::Bound(target);
        ready.submit_window = submit_window;
        Ok(())
    }

    pub(crate) fn ready_submit_window(&self) -> Option<KmsSubmitWindow> {
        self.ready.as_ref().map(|frame| frame.submit_window)
    }

    pub(crate) fn pending_frame_mut(&mut self) -> Option<&mut RenderedOutputFrame> {
        self.pending.as_mut().map(|pending| &mut pending.frame)
    }

    pub(crate) fn ready_render_fence_is_signaled(&self) -> io::Result<bool> {
        self.ready
            .as_ref()
            .ok_or_else(|| io::Error::other("no rendered output frame is ready"))?
            .render_fence
            .is_signaled_nonblocking()
    }

    pub(crate) fn ready_render_fence_fd(&self) -> Option<RawFd> {
        self.ready
            .as_ref()?
            .render_fence
            .readiness_fd()
            .map(AsRawFd::as_raw_fd)
    }

    pub(crate) fn duplicate_ready_render_completion_fd(&self) -> io::Result<OwnedFd> {
        self.ready
            .as_ref()
            .ok_or_else(|| io::Error::other("no rendered output frame is ready"))?
            .render_fence
            .duplicate_completion_fd()
    }

    pub(crate) fn pending_timing_fd(&self) -> Option<RawFd> {
        self.pending
            .as_ref()?
            .frame
            .render_fence
            .timing_fd()
            .map(AsRawFd::as_raw_fd)
    }

    pub(crate) fn ready_slot(&self) -> Option<OutputSlotId> {
        self.ready.as_ref().map(|ready| ready.slot)
    }

    pub(crate) fn ready_transaction_id(&self) -> Option<OutputTransactionId> {
        self.ready.as_ref().map(|ready| ready.transaction_id)
    }

    pub(crate) const fn rendering_slot(&self) -> Option<OutputSlotId> {
        self.rendering
    }

    pub(crate) fn quarantine_slot_id(&self) -> Option<OutputSlotId> {
        self.quarantine.as_ref().map(|quarantine| quarantine.slot)
    }

    pub(crate) fn is_poisoned(&self) -> bool {
        self.quarantine.is_some()
    }

    pub(crate) fn free_slot_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| self.slot_is_free(*slot))
            .count()
    }

    pub(crate) fn validate_invariants(&self) -> io::Result<()> {
        let roles = [
            Some(self.current),
            self.worker_queued_slot(),
            self.pending_slot(),
            self.ready_slot(),
            self.rendering,
            self.quarantine_slot_id(),
        ];
        let occupied: Vec<_> = roles.into_iter().flatten().collect();
        let mut occupied = occupied;
        occupied.extend(
            self.suspended
                .iter()
                .flatten()
                .map(|suspended| suspended.slot),
        );
        if occupied.iter().any(|slot| !self.slots.contains(*slot)) {
            return Err(io::Error::other(
                "output role references a slot outside the explicit pool",
            ));
        }
        for (index, slot) in occupied.iter().enumerate() {
            if occupied[index + 1..].contains(slot) {
                return Err(io::Error::other("explicit output slot roles alias"));
            }
        }
        if occupied.len() > EXPLICIT_OUTPUT_SLOT_CAPACITY {
            return Err(io::Error::other(
                "explicit output ownership exceeds three slots",
            ));
        }
        if self.ready.is_some() && self.rendering.is_some() {
            return Err(io::Error::other(
                "more than one composited primary is prepared",
            ));
        }
        if self.pending.is_some()
            && self.worker_queued.is_some()
            && (self.ready.is_some() || self.rendering.is_some())
        {
            return Err(io::Error::other(
                "pending plus worker-queued-next cannot own a third future primary",
            ));
        }
        for frame in [
            self.pending.as_ref().map(|pending| &pending.frame),
            self.worker_queued.as_ref().map(|queued| &queued.frame),
            self.ready.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if frame.pool_generation != self.pool_generation {
                return Err(io::Error::other(
                    "output frame belongs to an old swapchain generation",
                ));
            }
            if let Some(target) = frame.bound_target() {
                self.validate_target_claim(target)?;
                if target.clock_generation != self.pool_generation
                    || target.physical_claim().clock_generation != self.pool_generation
                {
                    return Err(io::Error::other(
                        "output frame belongs to an old presentation generation",
                    ));
                }
            }
        }
        for suspended in self.suspended.iter().flatten() {
            if suspended.pool_generation != self.pool_generation {
                return Err(io::Error::other(
                    "suspended output slot belongs to an old swapchain generation",
                ));
            }
            if suspended.abandoned_frame.as_ref().is_some_and(|frame| {
                frame.pool_generation != self.pool_generation || frame.slot != suspended.slot
            }) {
                return Err(io::Error::other(
                    "suspended output slot frame belongs to an invalid owner",
                ));
            }
        }
        let frontier = PresentationOpportunityFrontier::from_claims(
            [
                self.pending
                    .as_ref()
                    .and_then(|pending| pending.frame.bound_target()),
                self.worker_queued
                    .as_ref()
                    .and_then(|queued| queued.frame.bound_target()),
                self.ready.as_ref().and_then(|ready| ready.bound_target()),
            ]
            .into_iter()
            .flatten()
            .map(|target| target.physical_claim().opportunity_id()),
        )
        .map_err(|error| {
            io::Error::other(format!("presentation opportunity frontier: {error:?}"))
        })?;
        let _latest_claim = frontier.latest();
        self.validate_live_future_primary_claims()
    }

    pub(crate) fn validate_invariants_for(
        &self,
        pacing_mode: NativeOutputPacingMode,
    ) -> io::Result<()> {
        self.validate_invariants_for_limit(
            u8::from(pacing_mode == NativeOutputPacingMode::PredictiveTriple) + 1,
        )
    }

    pub(crate) fn validate_invariants_for_limit(&self, future_primary_limit: u8) -> io::Result<()> {
        self.validate_invariants()?;
        if future_primary_limit < 2
            && (self.pending.is_some() || self.worker_queued.is_some())
            && (self.ready.is_some() || self.rendering.is_some())
        {
            return Err(io::Error::other(
                "ReactiveDouble cannot own a ready or rendering slot while pageflip is pending",
            ));
        }
        Ok(())
    }

    fn quarantine_slot(
        &mut self,
        slot: OutputSlotId,
        timing_fence: Option<OwnedFd>,
        reason: OutputQuarantineReason,
        abandoned_frame: Option<RenderedOutputFrame>,
    ) -> io::Result<()> {
        if self.quarantine.is_some() {
            return Err(io::Error::other("an output slot is already quarantined"));
        }
        if self.suspended_slot_id(slot).is_some() {
            return Err(io::Error::other(
                "output slot is already owned by suspended output",
            ));
        }
        self.quarantine = Some(QuarantinedOutputSlot {
            slot,
            pool_generation: self.pool_generation,
            timing_fence,
            reason,
            abandoned_frame,
        });
        Ok(())
    }

    #[track_caller]
    fn ensure_operational(&self) -> io::Result<()> {
        if self.quarantine.is_some() {
            let caller = std::panic::Location::caller();
            return Err(io::Error::other(format!(
                "explicit output swapchain is quarantined and non-renderable at {}:{}",
                caller.file(),
                caller.line(),
            )));
        }
        if self.has_suspended_ownership() {
            return Err(io::Error::other(
                "explicit output swapchain has suspend-owned slots and is non-renderable",
            ));
        }
        Ok(())
    }

    fn ensure_suspendable(&self) -> io::Result<()> {
        if self.quarantine.is_some() {
            return Err(io::Error::other(
                "fatal output quarantine blocks session suspension recovery",
            ));
        }
        Ok(())
    }

    fn suspend_owned_slot(
        &mut self,
        suspended_index: usize,
        slot: OutputSlotId,
        timing_fence: Option<OwnedFd>,
        frame: RenderedOutputFrame,
    ) {
        assert!(
            self.suspended_slot_id(slot).is_none(),
            "output slot is already owned by suspended output"
        );
        let entry = self
            .suspended
            .get_mut(suspended_index)
            .expect("suspended output ownership index was preflighted");
        assert!(
            entry.is_none(),
            "suspended output ownership slot was preflighted as empty"
        );
        *entry = Some(SuspendedOutputSlot {
            slot,
            pool_generation: self.pool_generation,
            timing_fence,
            abandoned_frame: Some(frame),
        });
    }

    fn empty_suspended_slot_index(&self) -> io::Result<usize> {
        self.suspended
            .iter()
            .position(Option::is_none)
            .ok_or_else(|| {
                io::Error::other("suspended output ownership exceeds explicit pool capacity")
            })
    }

    fn suspended_slot_id(&self, slot: OutputSlotId) -> Option<OutputSlotId> {
        self.suspended
            .iter()
            .flatten()
            .find(|suspended| suspended.slot == slot)
            .map(|suspended| suspended.slot)
    }

    fn has_suspended_ownership(&self) -> bool {
        self.suspended.iter().any(Option::is_some)
    }

    fn suspended_slot_fence_signaled(&self, suspended: &SuspendedOutputSlot) -> io::Result<bool> {
        if suspended.timing_fence.is_none() {
            return suspended
                .abandoned_frame
                .as_ref()
                .map_or(Ok(true), |frame| {
                    frame.render_fence.is_signaled_nonblocking()
                });
        }
        let fence = suspended.timing_fence.as_ref().expect("checked above");
        poll_completion_fd(
            fence.as_raw_fd(),
            "suspended output render fence reported poll failure",
        )
    }

    fn validate_worker_queued_frame(&self, frame: &RenderedOutputFrame) -> io::Result<()> {
        let frame_target = frame
            .bound_target()
            .ok_or_else(|| io::Error::other("unbound output frame cannot enter worker queue"))?;
        if self.ready.as_ref().is_some_and(|ready| {
            ready.slot != frame.slot
                || ready.id != frame.id
                || ready.transaction_id != frame.transaction_id
                || ready.pool_generation != frame.pool_generation
                || ready.reservation != frame.reservation
        }) {
            return Err(io::Error::other(
                "worker-queued frame does not match ready ownership",
            ));
        }
        if frame.pool_generation != self.pool_generation
            || frame_target.clock_generation != self.pool_generation
            || frame_target.physical_claim().clock_generation != self.pool_generation
            || frame.slot == self.current
            || self.pending_slot() == Some(frame.slot)
            || self.quarantine_slot_id() == Some(frame.slot)
            || self.rendering == Some(frame.slot)
        {
            return Err(io::Error::other(
                "worker-queued frame identity aliases another output owner",
            ));
        }
        self.validate_target_claim(frame_target)?;
        if let Some(last_presented) = self.last_presented_primary_claim {
            validate_strictly_later_claim(last_presented, frame_target.physical_claim())?;
        }
        if let Some(pending) = &self.pending {
            validate_strictly_later_target(
                pending
                    .frame
                    .bound_target()
                    .ok_or_else(|| io::Error::other("pending output frame is unbound"))?,
                frame_target,
            )?;
        }
        Ok(())
    }

    fn validate_later_primary_target(&self, target: PresentationTarget) -> io::Result<()> {
        self.validate_target_claim(target)?;
        if let Some(last_presented) = self.last_presented_primary_claim {
            validate_strictly_later_claim(last_presented, target.physical_claim())?;
        }
        if let Some(worker) = &self.worker_queued {
            validate_strictly_later_target(
                worker
                    .frame
                    .bound_target()
                    .ok_or_else(|| io::Error::other("worker output frame is unbound"))?,
                target,
            )
        } else if let Some(pending) = &self.pending {
            validate_strictly_later_target(
                pending
                    .frame
                    .bound_target()
                    .ok_or_else(|| io::Error::other("pending output frame is unbound"))?,
                target,
            )
        } else {
            Ok(())
        }
    }

    fn validate_live_future_primary_claims(&self) -> io::Result<()> {
        if let Some(last_presented) = self.last_presented_primary_claim {
            if let Some(worker) = &self.worker_queued {
                validate_strictly_later_claim(
                    last_presented,
                    worker
                        .frame
                        .bound_target()
                        .ok_or_else(|| io::Error::other("worker output frame is unbound"))?
                        .physical_claim(),
                )?;
            }
            if let Some(pending) = &self.pending {
                validate_strictly_later_claim(
                    last_presented,
                    pending
                        .frame
                        .bound_target()
                        .ok_or_else(|| io::Error::other("pending output frame is unbound"))?
                        .physical_claim(),
                )?;
            }
            if let Some(ready) = &self.ready
                && let Some(target) = ready.bound_target()
            {
                validate_strictly_later_claim(last_presented, target.physical_claim())?;
            }
        }
        if let (Some(pending), Some(worker)) = (&self.pending, &self.worker_queued) {
            validate_strictly_later_target(
                pending
                    .frame
                    .bound_target()
                    .ok_or_else(|| io::Error::other("pending output frame is unbound"))?,
                worker
                    .frame
                    .bound_target()
                    .ok_or_else(|| io::Error::other("worker output frame is unbound"))?,
            )?;
        }
        if let Some(ready) = &self.ready
            && let Some(target) = ready.bound_target()
        {
            self.validate_later_primary_target(target)?;
        }
        Ok(())
    }

    fn validate_target_claim(&self, target: PresentationTarget) -> io::Result<()> {
        let claim = target.physical_claim();
        if claim.clock_generation != target.clock_generation {
            return Err(io::Error::other(
                "presentation target claim belongs to another clock generation",
            ));
        }
        if target.is_binding()
            && (claim.sequence != target.sequence
                || claim.presentation_time != target.presentation_time)
        {
            return Err(io::Error::other(
                "reserved presentation target claim does not match its target",
            ));
        }
        Ok(())
    }

    fn slot_is_free(&self, slot: OutputSlotId) -> bool {
        slot != self.current
            && self.worker_queued_slot() != Some(slot)
            && self.pending_slot() != Some(slot)
            && self.ready_slot() != Some(slot)
            && self.rendering != Some(slot)
            && self.quarantine_slot_id() != Some(slot)
            && self.suspended_slot_id(slot).is_none()
    }
}

fn poll_completion_fd(fd: RawFd, error_message: &str) -> io::Result<bool> {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut pollfd, 1, 0) };
    if ready < 0 {
        return Err(io::Error::last_os_error());
    }
    if pollfd.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
        return Err(io::Error::other(error_message));
    }
    Ok(ready > 0 && pollfd.revents & (libc::POLLIN | libc::POLLHUP) != 0)
}

fn validate_strictly_later_target(
    earlier: PresentationTarget,
    later: PresentationTarget,
) -> io::Result<()> {
    validate_strictly_later_claim(earlier.physical_claim(), later.physical_claim())
}

fn validate_strictly_later_claim(
    earlier: PrimaryRefreshClaim,
    later: PrimaryRefreshClaim,
) -> io::Result<()> {
    if earlier.clock_generation != later.clock_generation
        || later.sequence <= earlier.sequence
        || later.presentation_time <= earlier.presentation_time
    {
        return Err(io::Error::other(
            "later output primary physical claim is not strictly ordered",
        ));
    }
    Ok(())
}

fn is_strictly_later_claim(earlier: PrimaryRefreshClaim, later: PrimaryRefreshClaim) -> bool {
    earlier.clock_generation == later.clock_generation
        && later.sequence > earlier.sequence
        && later.presentation_time > earlier.presentation_time
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_output::presentation::plane::{
        FrozenCursorTestPolicy, FrozenPrimaryCursorPresentation, PresentedCursorDelivery,
    };
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn output_slot_linear_payload_scales_with_resolution_and_capacity() {
        for (width, height, one_slot, two_slots, three_slots) in [
            (1920, 1080, 8_294_400, 16_588_800, 24_883_200),
            (2560, 1440, 14_745_600, 29_491_200, 44_236_800),
            (3840, 2160, 33_177_600, 66_355_200, 99_532_800),
        ] {
            assert_eq!(output_slot_linear_payload_bytes(width, height), one_slot);
            assert_eq!(output_pool_linear_payload_bytes(width, height), three_slots);
            assert_eq!(
                output_pool_linear_payload_bytes(width, height) - one_slot,
                two_slots
            );
            assert_eq!(
                incremental_third_slot_linear_payload_bytes(width, height),
                one_slot
            );
        }
    }

    fn test_render_fence() -> NativeRenderFence {
        let mut pipe = [-1; 2];
        assert_eq!(
            unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        unsafe { libc::close(pipe[1]) };
        NativeRenderFence::from_submission_fd(unsafe { OwnedFd::from_raw_fd(pipe[0]) })
    }

    fn fd_is_open(fd: i32) -> bool {
        (unsafe { libc::fcntl(fd, libc::F_GETFD) }) >= 0
    }

    #[test]
    fn suspend_worker_completion_proof_stays_owned_on_token_error() {
        let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
            OutputSlotSet::new([
                OutputSlotId::new(0).unwrap(),
                OutputSlotId::new(1).unwrap(),
                OutputSlotId::new(2).unwrap(),
            ])
            .unwrap(),
            OutputSlotId::new(0).unwrap(),
            1,
        )
        .unwrap();
        let slot = swapchain.acquire_render_slot().unwrap();
        swapchain
            .finish_render(slot, 1, test_render_fence())
            .unwrap();
        let token = PageFlipToken::new(2).unwrap();
        let (completion_fence, _) = swapchain
            .take_ready_for_worker(token, MonotonicTimestampNs::new(1))
            .unwrap();
        let raw_fd = completion_fence.as_raw_fd();
        let mut completion_fence = Some(completion_fence);

        assert!(
            swapchain
                .suspend_abandon_worker_queued_with_completion_fence(
                    PageFlipToken::new(3).unwrap(),
                    &mut completion_fence,
                )
                .is_err()
        );
        assert!(fd_is_open(raw_fd));
    }

    #[test]
    fn returned_worker_input_fence_stays_owned_on_token_error() {
        let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
            OutputSlotSet::new([
                OutputSlotId::new(0).unwrap(),
                OutputSlotId::new(1).unwrap(),
                OutputSlotId::new(2).unwrap(),
            ])
            .unwrap(),
            OutputSlotId::new(0).unwrap(),
            1,
        )
        .unwrap();
        let slot = swapchain.acquire_render_slot().unwrap();
        swapchain
            .finish_render(slot, 1, test_render_fence())
            .unwrap();
        let token = PageFlipToken::new(4).unwrap();
        let (submission_fence, _) = swapchain
            .take_ready_for_worker(token, MonotonicTimestampNs::new(1))
            .unwrap();
        let raw_fd = submission_fence.as_raw_fd();
        let mut submission_fence = Some(submission_fence);

        assert!(
            swapchain
                .restore_worker_queued_submission_fence(
                    PageFlipToken::new(5).unwrap(),
                    &mut submission_fence,
                )
                .is_err()
        );
        assert!(fd_is_open(raw_fd));
    }

    #[test]
    fn returned_worker_input_fence_stays_owned_when_queue_is_missing() {
        let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
            OutputSlotSet::new([
                OutputSlotId::new(0).unwrap(),
                OutputSlotId::new(1).unwrap(),
                OutputSlotId::new(2).unwrap(),
            ])
            .unwrap(),
            OutputSlotId::new(0).unwrap(),
            1,
        )
        .unwrap();
        let mut fence = test_render_fence();
        let submission_fence = fence.take_submission_fd().unwrap();
        let raw_fd = submission_fence.as_raw_fd();
        let mut submission_fence = Some(submission_fence);

        assert!(
            swapchain
                .restore_worker_queued_submission_fence(
                    PageFlipToken::new(6).unwrap(),
                    &mut submission_fence,
                )
                .is_err()
        );
        assert!(submission_fence.is_some());
        assert!(fd_is_open(raw_fd));
    }

    #[test]
    fn suspend_worker_completion_proof_stays_owned_when_fatal_quarantine_blocks_suspend() {
        let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
            OutputSlotSet::new([
                OutputSlotId::new(0).unwrap(),
                OutputSlotId::new(1).unwrap(),
                OutputSlotId::new(2).unwrap(),
            ])
            .unwrap(),
            OutputSlotId::new(0).unwrap(),
            1,
        )
        .unwrap();
        let rendering_slot = swapchain.acquire_render_slot().unwrap();
        swapchain
            .quarantine_rendering(None, OutputQuarantineReason::AtomicSubmitFailure)
            .unwrap();
        assert_eq!(swapchain.quarantine_slot_id(), Some(rendering_slot));

        let mut fence = test_render_fence();
        let submission_fence = fence.take_submission_fd().unwrap();
        let raw_fd = submission_fence.as_raw_fd();
        let mut completion_fence = Some(submission_fence);
        assert!(
            swapchain
                .suspend_abandon_worker_queued_with_completion_fence(
                    PageFlipToken::new(7).unwrap(),
                    &mut completion_fence,
                )
                .is_err()
        );
        assert!(completion_fence.is_some());
        assert!(fd_is_open(raw_fd));
    }

    #[test]
    fn render_fence_timing_evidence_survives_timing_fd_take() {
        let slots = OutputSlotSet::new([
            OutputSlotId::new(0).expect("slot 0"),
            OutputSlotId::new(1).expect("slot 1"),
            OutputSlotId::new(2).expect("slot 2"),
        ])
        .expect("test slots");
        let swapchain = AtomicOutputSwapchain::from_presented_slots(
            slots,
            OutputSlotId::new(0).expect("current slot"),
            1,
        )
        .expect("test swapchain");
        let slot = OutputSlotId::new(1).expect("test slot");
        let target = test_target(1, 10, PresentationTargetReason::ForcedValidation);
        let mut frame = test_frame(&swapchain, slot, target);

        let first = frame
            .sample_fence_timing(MonotonicTimestampNs::new(20))
            .expect("sample timing")
            .expect("closed test pipe is observable");
        frame.mark_fence_timing_accounted();
        assert!(frame.render_fence.take_timing_fd().is_some());

        let second = frame
            .sample_fence_timing(MonotonicTimestampNs::new(30))
            .expect("reuse timing evidence")
            .expect("retained timing evidence");
        assert_eq!(second.signaled_at, first.signaled_at);
        assert_eq!(second.quality, first.quality);
        assert!(second.render_sample_recorded);
    }

    #[test]
    fn worker_suspend_retains_returned_submission_fence_when_timing_duplication_failed() {
        let slots = OutputSlotSet::new([
            OutputSlotId::new(0).expect("slot 0"),
            OutputSlotId::new(1).expect("slot 1"),
            OutputSlotId::new(2).expect("slot 2"),
        ])
        .expect("test slots");
        let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
            slots,
            OutputSlotId::new(0).expect("current slot"),
            1,
        )
        .expect("test swapchain");
        let slot = swapchain.acquire_render_slot().expect("render slot");
        let mut pipe = [-1; 2];
        assert_eq!(
            unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        let mut frame = test_frame(
            &swapchain,
            slot,
            test_target(1, 10, PresentationTargetReason::ReactiveDouble),
        );
        frame.render_fence =
            NativeRenderFence::from_submission_fd(unsafe { OwnedFd::from_raw_fd(pipe[0]) });
        let writer = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
        let _ = frame.render_fence.take_timing_fd();
        assert!(frame.render_fence.timing_fd().is_none());
        swapchain
            .finish_render_owned(frame)
            .expect("frame becomes ready");

        let token = PageFlipToken::new(100).expect("worker token");
        let submission_fence = swapchain
            .take_ready_for_worker(token, now(1))
            .expect("frame enters worker queue")
            .0;
        let mut submission_fence = Some(submission_fence);
        swapchain
            .suspend_abandon_worker_queued_with_completion_fence(token, &mut submission_fence)
            .expect("worker frame is retained for suspend");
        assert!(submission_fence.is_none());

        assert!(
            !swapchain
                .suspended_fences_signaled()
                .expect("returned submission fence is the completion proof")
        );
        assert!(swapchain.acquire_render_slot().is_err());
        assert!(swapchain.suspended_fence_fd().unwrap().is_some());
        drop(writer);
        assert!(swapchain.suspended_fences_signaled().unwrap());
        assert!(swapchain.take_suspended_frame().is_some());
        swapchain
            .recover_suspended_slot(true)
            .expect("suspended frame is now safe to retire");
        assert!(swapchain.acquire_render_slot().is_ok());
    }

    #[test]
    fn pending_frame_without_render_or_kms_completion_proof_is_not_releasable() {
        let slots = OutputSlotSet::new([
            OutputSlotId::new(0).expect("slot 0"),
            OutputSlotId::new(1).expect("slot 1"),
            OutputSlotId::new(2).expect("slot 2"),
        ])
        .expect("test slots");
        let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
            slots,
            OutputSlotId::new(0).expect("current slot"),
            1,
        )
        .expect("test swapchain");
        let slot = swapchain.acquire_render_slot().expect("render slot");
        let mut frame = test_frame(
            &swapchain,
            slot,
            test_target(1, 10, PresentationTargetReason::ReactiveDouble),
        );
        let _ = frame.render_fence.take_timing_fd();
        let _ = frame
            .render_fence
            .take_submission_fd()
            .expect("test submission fence");
        let token = PageFlipToken::new(101).expect("pageflip token");
        swapchain
            .submission_succeeded(frame, token, None, now(1), now(2))
            .expect("frame enters pending ownership");

        assert!(swapchain.pending_fence_signaled().is_err());
        assert_eq!(swapchain.pending_slot(), Some(slot));
    }

    #[test]
    fn pending_kms_out_fence_proves_render_ownership_when_timing_fd_is_missing() {
        let slots = OutputSlotSet::new([
            OutputSlotId::new(0).expect("slot 0"),
            OutputSlotId::new(1).expect("slot 1"),
            OutputSlotId::new(2).expect("slot 2"),
        ])
        .expect("test slots");
        let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
            slots,
            OutputSlotId::new(0).expect("current slot"),
            1,
        )
        .expect("test swapchain");
        let slot = swapchain.acquire_render_slot().expect("render slot");
        let mut frame = test_frame(
            &swapchain,
            slot,
            test_target(1, 10, PresentationTargetReason::ReactiveDouble),
        );
        let _ = frame.render_fence.take_timing_fd();
        let _ = frame
            .render_fence
            .take_submission_fd()
            .expect("test submission fence");
        let mut out_pipe = [-1; 2];
        assert_eq!(
            unsafe { libc::pipe2(out_pipe.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        let out_writer = unsafe { OwnedFd::from_raw_fd(out_pipe[1]) };
        swapchain
            .submission_succeeded(
                frame,
                PageFlipToken::new(102).expect("pageflip token"),
                Some(unsafe { OwnedFd::from_raw_fd(out_pipe[0]) }),
                now(1),
                now(2),
            )
            .expect("frame enters pending ownership");

        assert!(
            !swapchain
                .pending_fence_signaled()
                .expect("out-fence is a valid completion proof")
        );
        assert!(swapchain.pending_fence_fd().unwrap().is_some());
        drop(out_writer);
        assert!(swapchain.pending_fence_signaled().unwrap());
        assert!(swapchain.retire_pending_after_recovery().is_some());
    }

    #[test]
    fn sustained_predictive_worker_move_recycles_primary_and_cursor_owners() {
        fn render_move_frame(
            swapchain: &mut AtomicOutputSwapchain,
            cursor: &mut crate::native_output::output::NativeAtomicCursor,
            frame_number: u64,
        ) {
            let slot = swapchain
                .acquire_render_slot_for_limit(2)
                .expect("predictive triple buffering must admit one future frame");
            let target = predictive_test_target(frame_number, frame_number.saturating_mul(10));
            let mut frame = test_frame(swapchain, slot, target);
            frame.render_generation = frame_number;
            cursor.set_position(
                i32::try_from(frame_number).expect("test pointer position fits"),
                40,
            );
            let mut cursor_state = cursor.desired().clone();
            cursor_state.visible = true;
            cursor_state.framebuffer_id = Some(91);
            frame.frozen_cursor_plan = FrozenPrimaryCursorPlan {
                delivery: PresentedCursorDelivery::Hardware,
                primary_presentation: FrozenPrimaryCursorPresentation::Preserve,
                cursor_test_policy: FrozenCursorTestPolicy::Skip,
            };
            frame.frozen_cursor_plane_owner = Some(FrozenCursorPlaneOwner {
                revision: cursor.desired_revision(),
                client_source_key: None,
                capability_key: None,
                pin: Some(
                    cursor
                        .pin_framebuffer_for(&cursor_state)
                        .expect("cursor framebuffer must remain owned while rendering"),
                ),
            });
            swapchain
                .finish_render_owned(frame)
                .expect("rendered move frame becomes ready");
        }

        let slots = OutputSlotSet::new([
            OutputSlotId::new(0).expect("slot 0"),
            OutputSlotId::new(1).expect("slot 1"),
            OutputSlotId::new(2).expect("slot 2"),
        ])
        .expect("test slots");
        let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
            slots,
            OutputSlotId::new(0).expect("current slot"),
            1,
        )
        .expect("test swapchain");
        let mut cursor = crate::native_output::output::test_cursor_for_worker();
        let iterations = 512_u64;
        let mut latest_presented_geometry = 0;

        for frame_number in 1..=iterations {
            if frame_number == 1 {
                render_move_frame(&mut swapchain, &mut cursor, frame_number);
            }

            let token =
                PageFlipToken::new(frame_number.saturating_add(10_000)).expect("worker token");
            let (submission_fence, cursor_owner) = swapchain
                .take_ready_for_worker(token, now(frame_number.saturating_mul(10)))
                .expect("ready primary enters worker queue");
            drop(submission_fence);
            let cursor_owner = cursor_owner.expect("primary carries frozen cursor owner");
            assert_eq!(
                cursor_owner.pin.as_ref().map(|pin| pin.framebuffer_id()),
                Some(FramebufferId::new(91).unwrap())
            );

            if frame_number < iterations {
                render_move_frame(&mut swapchain, &mut cursor, frame_number + 1);
            }
            swapchain
                .validate_invariants_for(NativeOutputPacingMode::PredictiveTriple)
                .expect("worker queued primary plus one ready future remains bounded");

            swapchain
                .promote_worker_queued(
                    token,
                    None,
                    now(frame_number.saturating_mul(10).saturating_add(1)),
                    now(frame_number.saturating_mul(10).saturating_add(2)),
                )
                .expect("worker submission promotes exact primary");
            let completed = swapchain
                .complete_pageflip(token, 1)
                .expect("exact primary pageflip completes");
            assert_eq!(completed.frame.render_generation, frame_number);
            latest_presented_geometry = completed.frame.render_generation;
            swapchain
                .note_physical_primary_presentation(
                    completed
                        .frame
                        .bound_target()
                        .expect("bound target")
                        .physical_claim(),
                )
                .expect("physical primary claim remains ordered");
            drop(cursor_owner);
            swapchain
                .validate_invariants_for(NativeOutputPacingMode::PredictiveTriple)
                .expect("completed primary releases its slot exactly once");
        }

        assert_eq!(latest_presented_geometry, iterations);
        assert_eq!(swapchain.worker_queued_token(), None);
        assert_eq!(swapchain.pending_token(), None);
        assert_eq!(swapchain.ready_identity(), None);
        assert_eq!(swapchain.free_slot_count(), 2);
        assert!(swapchain.acquire_render_slot_for_limit(2).is_ok());
    }

    fn test_target(
        sequence: u64,
        presentation_time: u64,
        reason: PresentationTargetReason,
    ) -> PresentationTarget {
        let presentation_time = MonotonicTimestampNs::new(presentation_time);
        PresentationTarget {
            sequence,
            presentation_time,
            submit_not_before: presentation_time,
            render_start_deadline: presentation_time,
            refresh_interval: std::time::Duration::from_nanos(10),
            reason,
            clock_generation: 1,
            estimated: false,
            predicted_unreachable: false,
            physical_claim: oblivion_one::native::presentation_deadline::PrimaryRefreshClaim {
                sequence,
                presentation_time,
                clock_generation: 1,
            },
            selection_evidence: Default::default(),
        }
    }

    fn test_frame(
        swapchain: &AtomicOutputSwapchain,
        slot: OutputSlotId,
        target: PresentationTarget,
    ) -> RenderedOutputFrame {
        let frame_id = swapchain.next_frame_id();
        let now = MonotonicTimestampNs::new(frame_id);
        static NEXT_BATCH: AtomicU64 = AtomicU64::new(1);
        let batch_id = CompositorFrameBatchId::new(
            std::num::NonZeroU64::new(NEXT_BATCH.fetch_add(1, Ordering::Relaxed))
                .expect("test batch ID is nonzero"),
        );
        RenderedOutputFrame {
            id: frame_id,
            transaction_id: OutputTransactionId::new(
                std::num::NonZeroU64::new(frame_id).expect("test transaction ID is nonzero"),
            ),
            slot,
            framebuffer_id: FramebufferId::new(
                frame_id.try_into().expect("test framebuffer ID fits"),
            )
            .expect("test framebuffer ID is nonzero"),
            render_generation: 1,
            pool_generation: 1,
            reservation: FramePresentationReservation::Bound(target),
            submit_window: KmsSubmitWindow::try_new(
                target.presentation_time.get(),
                target.submit_not_before().get(),
                0,
                0,
            )
            .expect("test submit window"),
            render_fence: test_render_fence(),
            fence_timing_evidence: None,
            scene_commit: EglSceneFrameCommit::empty_for_test(),
            surface_damage: SurfaceDamagePresentation::default(),
            protocol_batch_id: batch_id,
            composite_started_at: now,
            fence_exported_at: now,
            rendered_at: now,
            client_commit_ns: None,
            callback_reaction_ns: None,
            callback_admission_ns: None,
            callback_surface_id: None,
            hardware_cursor_surface_id: None,
            cpu_prepass_duration_ns: 0,
            cpu_encode_duration_ns: 0,
            frozen_cursor_plan: FrozenPrimaryCursorPlan {
                delivery: PresentedCursorDelivery::Hidden,
                primary_presentation: FrozenPrimaryCursorPresentation::Preserve,
                cursor_test_policy: FrozenCursorTestPolicy::Skip,
            },
            frozen_cursor_plane_owner: None,
            frozen_cursor_trace_reveal: None,
            o1_admission: None,
        }
    }

    fn predictive_test_target(sequence: u64, presentation_time: u64) -> PresentationTarget {
        let mut target = test_target(
            sequence,
            presentation_time,
            PresentationTargetReason::PredictedPressure,
        );
        target.refresh_interval = std::time::Duration::from_nanos(6_060_606);
        target.submit_not_before =
            MonotonicTimestampNs::new(presentation_time.saturating_sub(500_000));
        target.render_start_deadline = target.submit_not_before;
        target
    }

    fn test_deferred_frame(
        swapchain: &AtomicOutputSwapchain,
        slot: OutputSlotId,
        predicted_target: PresentationTarget,
        predecessor: O1PredecessorAnchor,
    ) -> RenderedOutputFrame {
        let intent = O1PrepareIntent::from_target(
            predicted_target,
            swapchain.pool_generation(),
            predecessor,
        );
        let mut frame = test_frame(swapchain, slot, predicted_target);
        frame.reservation = FramePresentationReservation::DeferredO1(intent);
        frame
    }

    fn complete_physical_predecessor(
        swapchain: &mut AtomicOutputSwapchain,
        token: PageFlipToken,
        claim: PrimaryRefreshClaim,
    ) {
        swapchain
            .complete_pageflip(token, 1)
            .expect("predecessor pageflip");
        swapchain
            .note_physical_primary_presentation(claim)
            .expect("predecessor physical claim");
    }

    #[test]
    fn advisory_predecessor_claim_allows_reserved_o1_successor_at_swapchain_boundary() {
        let slots = OutputSlotSet::new([
            OutputSlotId::new(0).expect("slot 0"),
            OutputSlotId::new(1).expect("slot 1"),
            OutputSlotId::new(2).expect("slot 2"),
        ])
        .expect("test slots");
        let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
            slots,
            OutputSlotId::new(0).expect("current slot"),
            1,
        )
        .expect("test swapchain");

        let predecessor_slot = swapchain.acquire_render_slot().expect("predecessor slot");
        let mut predecessor = test_target(4, 40, PresentationTargetReason::ReactiveDouble);
        predecessor.physical_claim = PrimaryRefreshClaim {
            sequence: 2,
            presentation_time: MonotonicTimestampNs::new(20),
            clock_generation: 1,
        };
        swapchain
            .finish_render_owned(test_frame(&swapchain, predecessor_slot, predecessor))
            .expect("predecessor becomes ready");
        swapchain
            .submit_ready(PageFlipToken::new(10).expect("predecessor token"), None)
            .expect("predecessor submits");

        let successor_slot = swapchain.acquire_render_slot().expect("successor slot");
        let mut successor = test_target(3, 30, PresentationTargetReason::PredictedPressure);
        successor.physical_claim = PrimaryRefreshClaim {
            sequence: 3,
            presentation_time: MonotonicTimestampNs::new(30),
            clock_generation: 1,
        };

        swapchain
            .finish_render_owned(test_frame(&swapchain, successor_slot, successor))
            .expect("advisory metadata must not reject physical claim order");
        swapchain
            .validate_invariants()
            .expect("claim-ordered predecessor and successor");
    }

    #[test]
    fn deferred_o1_binds_to_first_successor_after_one_interval_miss() {
        let predecessor_claim = PrimaryRefreshClaim {
            sequence: 1,
            presentation_time: MonotonicTimestampNs::new(6_060_606),
            clock_generation: 1,
        };
        let mut predecessor_target = predictive_test_target(1, 6_060_606);
        predecessor_target.physical_claim = predecessor_claim;
        let (mut swapchain, predecessor_token) =
            swapchain_with_submitted_target(predecessor_target);
        let predecessor = swapchain
            .deferred_o1_predecessor()
            .expect("pending predecessor anchor");
        let slot = swapchain
            .acquire_render_slot()
            .expect("deferred render slot");
        swapchain
            .finish_render_owned(test_deferred_frame(
                &swapchain,
                slot,
                predictive_test_target(2, 12_121_212),
                predecessor,
            ))
            .expect("deferred frame becomes ready");
        assert!(swapchain.ready_is_deferred_o1());

        complete_physical_predecessor(
            &mut swapchain,
            predecessor_token,
            PrimaryRefreshClaim {
                sequence: 2,
                presentation_time: MonotonicTimestampNs::new(12_121_212),
                clock_generation: 1,
            },
        );
        let (transaction_id, target, submit_window, advanced_intervals) = swapchain
            .deferred_o1_binding_candidate(1, now(12_200_000))
            .expect("binding candidate lookup")
            .expect("deferred frame should bind");
        assert_eq!(target.physical_claim().sequence, 3);
        assert_eq!(advanced_intervals, 0);
        swapchain
            .commit_deferred_o1_binding(transaction_id, target, submit_window)
            .expect("deferred frame binds once");
        assert!(!swapchain.ready_is_deferred_o1());
        assert_eq!(swapchain.ready_identity().unwrap().target, Some(target));
        assert!(
            swapchain
                .commit_deferred_o1_binding(transaction_id, target, submit_window)
                .is_err()
        );
    }

    #[test]
    fn deferred_o1_target_mutation_preserves_physical_frame_key() {
        let predecessor_target = predictive_test_target(1, 6_060_606);
        let (mut swapchain, predecessor_token) =
            swapchain_with_submitted_target(predecessor_target);
        let predecessor = swapchain
            .deferred_o1_predecessor()
            .expect("pending predecessor anchor");
        let slot = swapchain
            .acquire_render_slot()
            .expect("deferred render slot");
        swapchain
            .finish_render_owned(test_deferred_frame(
                &swapchain,
                slot,
                predictive_test_target(2, 12_121_212),
                predecessor,
            ))
            .expect("deferred frame becomes ready");

        let before_state = swapchain.ready_identity().expect("deferred ready state");
        let before_key = OutputFrameKey::from(&before_state);
        complete_physical_predecessor(
            &mut swapchain,
            predecessor_token,
            PrimaryRefreshClaim {
                sequence: 2,
                presentation_time: MonotonicTimestampNs::new(12_121_212),
                clock_generation: 1,
            },
        );
        let (transaction_id, target, submit_window, _) = swapchain
            .deferred_o1_binding_candidate(1, now(12_200_000))
            .expect("binding candidate lookup")
            .expect("deferred frame should bind");
        swapchain
            .commit_deferred_o1_binding(transaction_id, target, submit_window)
            .expect("deferred frame binds");

        let after_state = swapchain.ready_identity().expect("bound ready state");
        assert_ne!(before_state, after_state);
        assert_eq!(before_key, OutputFrameKey::from(&after_state));
        assert_eq!(
            before_key,
            OutputFrameKey::from(swapchain.ready.as_ref().expect("bound ready frame"))
        );
    }

    #[test]
    fn deferred_o1_skips_an_already_impossible_successor() {
        let predecessor_target = predictive_test_target(1, 6_060_606);
        let (mut swapchain, predecessor_token) =
            swapchain_with_submitted_target(predecessor_target);
        let predecessor = swapchain
            .deferred_o1_predecessor()
            .expect("pending predecessor anchor");
        let slot = swapchain
            .acquire_render_slot()
            .expect("deferred render slot");
        swapchain
            .finish_render_owned(test_deferred_frame(
                &swapchain,
                slot,
                predictive_test_target(2, 12_121_212),
                predecessor,
            ))
            .expect("deferred frame becomes ready");
        complete_physical_predecessor(
            &mut swapchain,
            predecessor_token,
            PrimaryRefreshClaim {
                sequence: 2,
                presentation_time: MonotonicTimestampNs::new(12_121_212),
                clock_generation: 1,
            },
        );

        let (_, target, _, advanced_intervals) = swapchain
            .deferred_o1_binding_candidate(1, now(18_200_000))
            .expect("binding candidate lookup")
            .expect("deferred frame should skip stale successor");
        assert_eq!(target.physical_claim().sequence, 4);
        assert_eq!(advanced_intervals, 1);
    }

    #[test]
    fn deferred_o1_survives_multi_refresh_predecessor_miss() {
        let predecessor_target = predictive_test_target(1, 6_060_606);
        let predecessor_claim = PrimaryRefreshClaim {
            sequence: 3,
            presentation_time: MonotonicTimestampNs::new(18_181_818),
            clock_generation: 1,
        };
        let (mut swapchain, predecessor_token) =
            swapchain_with_submitted_target(predecessor_target);
        let predecessor = swapchain
            .deferred_o1_predecessor()
            .expect("pending predecessor anchor");
        let slot = swapchain
            .acquire_render_slot()
            .expect("deferred render slot");
        swapchain
            .finish_render_owned(test_deferred_frame(
                &swapchain,
                slot,
                predictive_test_target(2, 12_121_212),
                predecessor,
            ))
            .expect("deferred frame becomes ready");

        complete_physical_predecessor(&mut swapchain, predecessor_token, predecessor_claim);
        let (_, target, _, advanced_intervals) = swapchain
            .deferred_o1_binding_candidate(1, now(18_200_000))
            .expect("binding candidate lookup")
            .expect("deferred frame should bind after a multi-refresh miss");
        assert_eq!(target.physical_claim().sequence, 4);
        assert_eq!(advanced_intervals, 0);
    }

    #[test]
    fn deferred_o1_waits_for_live_pending_predecessor() {
        let slots = OutputSlotSet::new([
            OutputSlotId::new(0).expect("slot 0"),
            OutputSlotId::new(1).expect("slot 1"),
            OutputSlotId::new(2).expect("slot 2"),
        ])
        .expect("test slots");
        let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
            slots,
            OutputSlotId::new(0).expect("current slot"),
            1,
        )
        .expect("test swapchain");

        let first_slot = swapchain.acquire_render_slot().expect("first slot");
        swapchain
            .finish_render_owned(test_frame(
                &swapchain,
                first_slot,
                predictive_test_target(1, 6_060_606),
            ))
            .expect("first frame ready");
        let first_token = PageFlipToken::new(10).expect("first token");
        swapchain
            .submit_ready(first_token, None)
            .expect("first frame submits");
        complete_physical_predecessor(
            &mut swapchain,
            first_token,
            PrimaryRefreshClaim {
                sequence: 1,
                presentation_time: MonotonicTimestampNs::new(6_060_606),
                clock_generation: 1,
            },
        );

        let predecessor_slot = swapchain.acquire_render_slot().expect("predecessor slot");
        swapchain
            .finish_render_owned(test_frame(
                &swapchain,
                predecessor_slot,
                predictive_test_target(2, 12_121_212),
            ))
            .expect("predecessor ready");
        let predecessor_token = PageFlipToken::new(11).expect("predecessor token");
        swapchain
            .submit_ready(predecessor_token, None)
            .expect("predecessor submits");
        let predecessor = swapchain
            .deferred_o1_predecessor()
            .expect("live pending predecessor");

        let successor_slot = swapchain.acquire_render_slot().expect("successor slot");
        swapchain
            .finish_render_owned(test_deferred_frame(
                &swapchain,
                successor_slot,
                predictive_test_target(3, 18_181_818),
                predecessor,
            ))
            .expect("successor becomes ready before predecessor pageflip");

        assert_eq!(swapchain.deferred_o1_binding_failure(1), None);
        assert_eq!(
            swapchain.deferred_o1_binding_readiness(1),
            DeferredO1BindingReadiness::WaitingForPredecessor
        );
        let ready_identity = swapchain
            .ready_identity()
            .expect("ready successor identity");
        for attempt in 0..3 {
            assert_eq!(
                swapchain.deferred_o1_binding_readiness(1),
                DeferredO1BindingReadiness::WaitingForPredecessor
            );
            assert_eq!(swapchain.deferred_o1_binding_failure(1), None);
            assert!(
                swapchain
                    .deferred_o1_binding_candidate(1, now(18_000_000 + attempt))
                    .expect("waiting candidate lookup")
                    .is_none()
            );
            assert_eq!(swapchain.ready_identity(), Some(ready_identity));
        }
        assert!(swapchain.ready_is_deferred_o1());
        assert!(
            swapchain
                .ready_identity()
                .expect("ready successor")
                .target
                .is_none()
        );
        assert!(
            swapchain
                .take_ready_for_worker(PageFlipToken::new(12).expect("worker token"), now(1))
                .is_err()
        );
        assert!(swapchain.take_ready_for_submission().is_err());
        assert!(
            swapchain
                .suspend_abandon_ready()
                .expect("waiting successor terminal settlement")
        );
        assert!(swapchain.ready_identity().is_none());
        assert_eq!(swapchain.quarantine_slot_id(), None);
        assert!(
            swapchain
                .take_suspended_frame()
                .expect("abandoned waiting successor")
                .is_deferred_o1()
        );
        swapchain
            .recover_suspended_slot(true)
            .expect("waiting successor suspend recovery");
        assert_eq!(swapchain.quarantine_slot_id(), None);
        assert_eq!(swapchain.pending_token(), Some(predecessor_token));
        assert_eq!(swapchain.worker_queued_token(), None);
    }

    #[test]
    fn deferred_o1_waits_for_live_worker_queued_predecessor() {
        let slots = OutputSlotSet::new([
            OutputSlotId::new(0).expect("slot 0"),
            OutputSlotId::new(1).expect("slot 1"),
            OutputSlotId::new(2).expect("slot 2"),
        ])
        .expect("test slots");
        let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
            slots,
            OutputSlotId::new(0).expect("current slot"),
            1,
        )
        .expect("test swapchain");

        let first_slot = swapchain.acquire_render_slot().expect("first slot");
        swapchain
            .finish_render_owned(test_frame(
                &swapchain,
                first_slot,
                predictive_test_target(1, 6_060_606),
            ))
            .expect("first frame ready");
        let first_token = PageFlipToken::new(20).expect("first token");
        swapchain
            .submit_ready(first_token, None)
            .expect("first frame submits");
        complete_physical_predecessor(
            &mut swapchain,
            first_token,
            PrimaryRefreshClaim {
                sequence: 1,
                presentation_time: MonotonicTimestampNs::new(6_060_606),
                clock_generation: 1,
            },
        );

        let predecessor_slot = swapchain.acquire_render_slot().expect("predecessor slot");
        swapchain
            .finish_render_owned(test_frame(
                &swapchain,
                predecessor_slot,
                predictive_test_target(2, 12_121_212),
            ))
            .expect("predecessor ready");
        let predecessor_token = PageFlipToken::new(21).expect("predecessor token");
        swapchain
            .take_ready_for_worker(predecessor_token, now(12_000_000))
            .expect("predecessor enters worker queue");
        let predecessor = swapchain
            .deferred_o1_predecessor()
            .expect("live worker-queued predecessor");

        let successor_slot = swapchain.acquire_render_slot().expect("successor slot");
        swapchain
            .finish_render_owned(test_deferred_frame(
                &swapchain,
                successor_slot,
                predictive_test_target(3, 18_181_818),
                predecessor,
            ))
            .expect("successor becomes ready before worker promotion");

        assert_eq!(
            swapchain.deferred_o1_binding_readiness(1),
            DeferredO1BindingReadiness::WaitingForPredecessor
        );
        assert!(
            swapchain
                .deferred_o1_binding_candidate(1, now(18_000_000))
                .expect("waiting candidate lookup")
                .is_none()
        );
        assert!(swapchain.ready_is_deferred_o1());
        assert_eq!(swapchain.worker_queued_token(), Some(predecessor_token));
    }

    #[test]
    fn deferred_o1_binds_when_predecessor_presents_before_render_completion() {
        let predecessor_claim = PrimaryRefreshClaim {
            sequence: 2,
            presentation_time: MonotonicTimestampNs::new(12_121_212),
            clock_generation: 1,
        };
        let (mut swapchain, predecessor_token) =
            swapchain_with_submitted_target(predictive_test_target(1, 6_060_606));
        let predecessor = swapchain
            .deferred_o1_predecessor()
            .expect("pending predecessor anchor");
        complete_physical_predecessor(&mut swapchain, predecessor_token, predecessor_claim);

        let slot = swapchain
            .acquire_render_slot()
            .expect("deferred render slot");
        swapchain
            .finish_render_owned(test_deferred_frame(
                &swapchain,
                slot,
                predictive_test_target(2, 12_121_212),
                predecessor,
            ))
            .expect("deferred render completes after predecessor pageflip");
        let (_, target, _, _) = swapchain
            .deferred_o1_binding_candidate(1, now(12_200_000))
            .expect("binding candidate lookup")
            .expect("deferred frame should bind after render completion");
        assert!(matches!(
            swapchain.deferred_o1_binding_readiness(1),
            DeferredO1BindingReadiness::Bindable { .. }
        ));
        assert_eq!(target.physical_claim().sequence, 3);
    }

    #[test]
    fn unbound_o1_is_not_an_overtake_owner_or_submit_candidate() {
        let (mut swapchain, _predecessor_token) =
            swapchain_with_submitted_target(predictive_test_target(1, 6_060_606));
        let predecessor = swapchain
            .deferred_o1_predecessor()
            .expect("pending predecessor anchor");
        let slot = swapchain
            .acquire_render_slot()
            .expect("deferred render slot");
        swapchain
            .finish_render_owned(test_deferred_frame(
                &swapchain,
                slot,
                predictive_test_target(2, 12_121_212),
                predecessor,
            ))
            .expect("deferred frame becomes ready");
        let actual_claim = PrimaryRefreshClaim {
            sequence: 2,
            presentation_time: MonotonicTimestampNs::new(12_121_212),
            clock_generation: 1,
        };
        assert_eq!(
            swapchain.revalidate_physical_primary_presentation(actual_claim),
            PhysicalPrimaryClaimRevalidation::Valid
        );
        assert!(
            swapchain
                .take_ready_for_worker(PageFlipToken::new(99).expect("worker token"), now(1))
                .is_err()
        );
        assert!(swapchain.take_ready_for_submission().is_err());
        assert!(swapchain.ready_is_deferred_o1());
    }

    #[test]
    fn deferred_o1_rejects_stale_predecessor_identity() {
        let (mut swapchain, predecessor_token) =
            swapchain_with_submitted_target(predictive_test_target(1, 6_060_606));
        let predecessor = swapchain
            .deferred_o1_predecessor()
            .expect("pending predecessor anchor");
        let slot = swapchain
            .acquire_render_slot()
            .expect("deferred render slot");
        swapchain
            .finish_render_owned(test_deferred_frame(
                &swapchain,
                slot,
                predictive_test_target(2, 12_121_212),
                predecessor,
            ))
            .expect("deferred frame becomes ready");
        let claim = PrimaryRefreshClaim {
            sequence: 2,
            presentation_time: MonotonicTimestampNs::new(12_121_212),
            clock_generation: 1,
        };
        complete_physical_predecessor(&mut swapchain, predecessor_token, claim);
        swapchain.last_presented_primary_anchor = Some((
            O1PredecessorAnchor {
                frame_id: predecessor.frame_id.saturating_add(1),
                ..predecessor
            },
            claim,
        ));
        assert_eq!(
            swapchain.deferred_o1_binding_readiness(1),
            DeferredO1BindingReadiness::Stale(DeferredO1BindingFailure::IdentityMismatch)
        );
        assert_eq!(
            swapchain.deferred_o1_binding_failure(1),
            Some(DeferredO1BindingFailure::IdentityMismatch)
        );
        assert!(swapchain.ready_is_deferred_o1());
    }

    #[test]
    fn deferred_o1_rejects_stale_output_generation() {
        let (mut swapchain, predecessor_token) =
            swapchain_with_submitted_target(predictive_test_target(1, 6_060_606));
        let predecessor = swapchain
            .deferred_o1_predecessor()
            .expect("pending predecessor anchor");
        let slot = swapchain
            .acquire_render_slot()
            .expect("deferred render slot");
        swapchain
            .finish_render_owned(test_deferred_frame(
                &swapchain,
                slot,
                predictive_test_target(2, 12_121_212),
                predecessor,
            ))
            .expect("deferred frame becomes ready");
        complete_physical_predecessor(
            &mut swapchain,
            predecessor_token,
            PrimaryRefreshClaim {
                sequence: 2,
                presentation_time: MonotonicTimestampNs::new(12_121_212),
                clock_generation: 1,
            },
        );
        assert_eq!(
            swapchain.deferred_o1_binding_failure(2),
            Some(DeferredO1BindingFailure::GenerationMismatch)
        );
        assert!(swapchain.ready_is_deferred_o1());
    }

    #[test]
    fn reserved_claim_order_is_rejected_even_when_metadata_looks_later() {
        let predecessor_claim = PrimaryRefreshClaim {
            sequence: 4,
            presentation_time: MonotonicTimestampNs::new(40),
            clock_generation: 1,
        };
        let (mut swapchain, _) = swapchain_with_submitted_claim(predecessor_claim);
        let successor_slot = swapchain.acquire_render_slot().expect("successor slot");
        let successor = test_target(3, 30, PresentationTargetReason::PredictedPressure);

        let error = swapchain
            .finish_render_owned(test_frame(&swapchain, successor_slot, successor))
            .expect_err("a reserved successor behind its predecessor must be rejected");
        assert!(error.to_string().contains("physical claim"));
    }

    #[test]
    fn duplicate_physical_claim_is_rejected() {
        let predecessor_claim = PrimaryRefreshClaim {
            sequence: 2,
            presentation_time: MonotonicTimestampNs::new(20),
            clock_generation: 1,
        };
        let (mut swapchain, _) = swapchain_with_submitted_claim(predecessor_claim);
        let successor_slot = swapchain.acquire_render_slot().expect("successor slot");
        let mut successor = test_target(3, 30, PresentationTargetReason::PredictedPressure);
        successor.physical_claim = predecessor_claim;

        let error = swapchain
            .finish_render_owned(test_frame(&swapchain, successor_slot, successor))
            .expect_err("two live frames cannot claim the same physical refresh");
        assert!(!error.to_string().is_empty());
    }

    #[test]
    fn physical_claim_revalidation_classifies_generation_and_regression_violations() {
        let claim = PrimaryRefreshClaim {
            sequence: 2,
            presentation_time: MonotonicTimestampNs::new(20),
            clock_generation: 1,
        };
        let (mut swapchain, predecessor_token) = swapchain_with_submitted_claim(claim);

        assert_eq!(
            swapchain.revalidate_physical_primary_presentation(PrimaryRefreshClaim {
                clock_generation: 2,
                ..claim
            }),
            PhysicalPrimaryClaimRevalidation::Fatal(
                PhysicalPrimaryClaimViolation::GenerationMismatch
            )
        );
        swapchain
            .complete_pageflip(predecessor_token, 1)
            .expect("predecessor pageflip");
        swapchain
            .note_physical_primary_presentation(claim)
            .expect("first physical claim");
        assert_eq!(
            swapchain.revalidate_physical_primary_presentation(claim),
            PhysicalPrimaryClaimRevalidation::Fatal(PhysicalPrimaryClaimViolation::Regression)
        );
    }

    #[test]
    fn pending_claim_allows_physical_pageflip_before_predicted_time() {
        let claim = PrimaryRefreshClaim {
            sequence: 1,
            presentation_time: MonotonicTimestampNs::new(20),
            clock_generation: 1,
        };
        let target = test_target(1, 20, PresentationTargetReason::Normal);
        let (mut swapchain, predecessor_token) = swapchain_with_submitted_target(target);

        let actual_claim = PrimaryRefreshClaim {
            presentation_time: MonotonicTimestampNs::new(10),
            ..claim
        };
        assert_eq!(
            swapchain.revalidate_physical_primary_presentation(actual_claim),
            PhysicalPrimaryClaimRevalidation::Valid
        );
        swapchain
            .complete_pageflip(predecessor_token, 1)
            .expect("pending predecessor pageflip");
        swapchain
            .note_physical_primary_presentation(actual_claim)
            .expect("first physical claim");
    }

    #[test]
    fn physical_predecessor_miss_revalidates_ready_without_mutating_its_claim() {
        let slots = OutputSlotSet::new([
            OutputSlotId::new(0).expect("slot 0"),
            OutputSlotId::new(1).expect("slot 1"),
            OutputSlotId::new(2).expect("slot 2"),
        ])
        .expect("test slots");
        let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
            slots,
            OutputSlotId::new(0).expect("current slot"),
            1,
        )
        .expect("test swapchain");

        let predecessor_slot = swapchain.acquire_render_slot().expect("predecessor slot");
        let mut predecessor = test_target(4, 40, PresentationTargetReason::ReactiveDouble);
        predecessor.physical_claim = PrimaryRefreshClaim {
            sequence: 2,
            presentation_time: MonotonicTimestampNs::new(20),
            clock_generation: 1,
        };
        swapchain
            .finish_render_owned(test_frame(&swapchain, predecessor_slot, predecessor))
            .expect("predecessor ready");
        let predecessor_token = PageFlipToken::new(20).expect("predecessor token");
        swapchain
            .submit_ready(predecessor_token, None)
            .expect("predecessor submitted");

        let successor_slot = swapchain.acquire_render_slot().expect("successor slot");
        let mut successor = test_target(3, 30, PresentationTargetReason::PredictedPressure);
        successor.physical_claim = PrimaryRefreshClaim {
            sequence: 3,
            presentation_time: MonotonicTimestampNs::new(30),
            clock_generation: 1,
        };
        swapchain
            .finish_render_owned(test_frame(&swapchain, successor_slot, successor))
            .expect("successor ready");
        let ready_before_miss = swapchain.ready_identity().expect("ready identity");
        swapchain
            .complete_pageflip(predecessor_token, 1)
            .expect("predecessor pageflip");

        let result = swapchain.revalidate_physical_primary_presentation(PrimaryRefreshClaim {
            sequence: 3,
            presentation_time: MonotonicTimestampNs::new(30),
            clock_generation: 1,
        });
        assert_eq!(
            result,
            PhysicalPrimaryClaimRevalidation::OvertakesReady {
                owner: ready_before_miss
            }
        );
        assert_eq!(swapchain.ready_identity(), Some(ready_before_miss));
    }

    #[test]
    fn physical_predecessor_miss_revalidates_worker_claim_without_duplicate_submit() {
        let slots = OutputSlotSet::new([
            OutputSlotId::new(0).expect("slot 0"),
            OutputSlotId::new(1).expect("slot 1"),
            OutputSlotId::new(2).expect("slot 2"),
        ])
        .expect("test slots");
        let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
            slots,
            OutputSlotId::new(0).expect("current slot"),
            1,
        )
        .expect("test swapchain");

        let predecessor_slot = swapchain.acquire_render_slot().expect("predecessor slot");
        let mut predecessor = test_target(4, 40, PresentationTargetReason::ReactiveDouble);
        predecessor.physical_claim = PrimaryRefreshClaim {
            sequence: 2,
            presentation_time: MonotonicTimestampNs::new(20),
            clock_generation: 1,
        };
        swapchain
            .finish_render_owned(test_frame(&swapchain, predecessor_slot, predecessor))
            .expect("predecessor ready");
        let predecessor_token = PageFlipToken::new(21).expect("predecessor token");
        swapchain
            .submit_ready(predecessor_token, None)
            .expect("predecessor submitted");

        let successor_slot = swapchain.acquire_render_slot().expect("successor slot");
        let mut successor = test_target(3, 30, PresentationTargetReason::PredictedPressure);
        successor.physical_claim = PrimaryRefreshClaim {
            sequence: 3,
            presentation_time: MonotonicTimestampNs::new(30),
            clock_generation: 1,
        };
        swapchain
            .finish_render_owned(test_frame(&swapchain, successor_slot, successor))
            .expect("successor ready");
        swapchain
            .take_ready_for_worker(PageFlipToken::new(22).expect("worker token"), now(1))
            .expect("successor worker queued");
        let worker_before_miss = swapchain.worker_queued_identity().expect("worker identity");

        let result = swapchain.revalidate_physical_primary_presentation(PrimaryRefreshClaim {
            sequence: 3,
            presentation_time: MonotonicTimestampNs::new(30),
            clock_generation: 1,
        });
        assert_eq!(
            result,
            PhysicalPrimaryClaimRevalidation::OvertakesWorkerQueued {
                owner: worker_before_miss
            }
        );
        assert_eq!(swapchain.worker_queued_identity(), Some(worker_before_miss));
    }

    #[test]
    fn quarantined_successor_does_not_block_predecessor_pageflip_completion() {
        let predecessor_claim = PrimaryRefreshClaim {
            sequence: 2,
            presentation_time: MonotonicTimestampNs::new(20),
            clock_generation: 1,
        };
        let (mut swapchain, predecessor_token) = swapchain_with_submitted_claim(predecessor_claim);
        let successor_slot = swapchain.acquire_render_slot().expect("successor slot");
        let mut successor = test_target(3, 30, PresentationTargetReason::PredictedPressure);
        successor.physical_claim = PrimaryRefreshClaim {
            sequence: 3,
            presentation_time: MonotonicTimestampNs::new(30),
            clock_generation: 1,
        };
        swapchain
            .finish_render_owned(test_frame(&swapchain, successor_slot, successor))
            .expect("successor ready");
        let successor_token = PageFlipToken::new(22).expect("successor token");
        swapchain
            .take_ready_for_worker(successor_token, now(1))
            .expect("successor worker queued");
        swapchain
            .suspend_abandon_worker_queued(successor_token)
            .expect("successor quarantined");

        swapchain
            .complete_pageflip(predecessor_token, 1)
            .expect("predecessor pageflip remains completable");
    }

    fn swapchain_with_submitted_claim(
        claim: PrimaryRefreshClaim,
    ) -> (AtomicOutputSwapchain, PageFlipToken) {
        let mut predecessor = test_target(
            claim.sequence,
            claim.presentation_time.get(),
            PresentationTargetReason::PredictedPressure,
        );
        predecessor.physical_claim = claim;
        swapchain_with_submitted_target(predecessor)
    }

    fn swapchain_with_submitted_target(
        predecessor: PresentationTarget,
    ) -> (AtomicOutputSwapchain, PageFlipToken) {
        let slots = OutputSlotSet::new([
            OutputSlotId::new(0).expect("slot 0"),
            OutputSlotId::new(1).expect("slot 1"),
            OutputSlotId::new(2).expect("slot 2"),
        ])
        .expect("test slots");
        let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
            slots,
            OutputSlotId::new(0).expect("current slot"),
            1,
        )
        .expect("test swapchain");
        let predecessor_slot = swapchain.acquire_render_slot().expect("predecessor slot");
        swapchain
            .finish_render_owned(test_frame(&swapchain, predecessor_slot, predecessor))
            .expect("predecessor ready");
        let token = PageFlipToken::new(30).expect("predecessor token");
        swapchain
            .submit_ready(token, None)
            .expect("predecessor submits");
        (swapchain, token)
    }

    const fn now(value: u64) -> MonotonicTimestampNs {
        MonotonicTimestampNs::new(value)
    }
}
