use std::{
    io,
    os::fd::{AsRawFd, OwnedFd, RawFd},
};

#[cfg(test)]
use std::num::NonZeroU64;

use oblivion_one::compositor::{CompositorFrameBatchId, SurfaceDamagePresentation};
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
use crate::native_output::presentation::{
    kms_timing::KmsSubmitWindow, plane::FrozenPrimaryCursorPlan, plane_policy::CursorCapabilityKey,
};
use oblivion_one::native::buffering::PresentationOpportunityFrontier;

pub(crate) const EXPLICIT_OUTPUT_SLOT_CAPACITY: usize = 3;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputQuarantineReason {
    PostDrawRenderFailure,
    RenderFenceExportFailure,
    AtomicSubmitFailure,
    SuspendAbandonment,
}

impl OutputQuarantineReason {
    const fn is_fatal(self) -> bool {
        !matches!(self, Self::SuspendAbandonment)
    }
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
pub(crate) struct RenderedOutputFrame {
    pub(crate) id: u64,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) slot: OutputSlotId,
    pub(crate) framebuffer_id: FramebufferId,
    pub(crate) render_generation: u64,
    pub(crate) pool_generation: u64,
    pub(crate) target: PresentationTarget,
    pub(crate) submit_window: KmsSubmitWindow,
    pub(crate) render_fence: NativeRenderFence,
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
    pub(crate) cpu_prepass_duration_ns: u64,
    pub(crate) cpu_encode_duration_ns: u64,
    pub(crate) frozen_cursor_plan: FrozenPrimaryCursorPlan,
    pub(crate) frozen_cursor_plane_owner: Option<FrozenCursorPlaneOwner>,
    pub(crate) o1_admission: Option<O1AdmissionObservation>,
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
    pub(crate) target: PresentationTarget,
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
            target: frame.target,
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
    next_frame_id: u64,
    presentation_serial: u64,
    current_framebuffer_id: Option<FramebufferId>,
    last_presented_primary_claim: Option<PrimaryRefreshClaim>,
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
            next_frame_id: 1,
            presentation_serial: 0,
            current_framebuffer_id: None,
            last_presented_primary_claim: None,
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
            target,
            submit_window: KmsSubmitWindow::try_new(
                target.presentation_time.get(),
                target.submit_not_before().get(),
                0,
                0,
            )
            .expect("test output frame has a reachable submit window"),
            render_fence,
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
        self.validate_later_primary_target(frame.target)?;
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
            target: PresentationTarget {
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
            },
            submit_window: KmsSubmitWindow::try_new(now.get(), now.get(), 0, 0)
                .expect("test ready frame has a reachable submit window"),
            render_fence,
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
            cpu_prepass_duration_ns: 0,
            cpu_encode_duration_ns: 0,
            frozen_cursor_plan,
            frozen_cursor_plane_owner,
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
        submission_fence: OwnedFd,
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
        let Some(mut queued) = self.worker_queued.take() else {
            return Ok(false);
        };
        if queued.token != token {
            self.worker_queued = Some(queued);
            return Err(io::Error::other(
                "re-planned worker output token does not match queued ownership",
            ));
        }
        if queued.frame.pool_generation != self.pool_generation {
            self.worker_queued = Some(queued);
            return Err(io::Error::other(
                "re-planned worker output frame belongs to an old pool generation",
            ));
        }
        if queued.frame.frozen_cursor_plane_owner.is_some() {
            self.worker_queued = Some(queued);
            return Err(io::Error::other(
                "re-planned worker output frame already owns a frozen cursor",
            ));
        }
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
        self.validate_later_primary_target(ready.target)?;
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
        self.validate_later_primary_target(frame.target)?;
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
        if self.quarantine.is_some() {
            return Err(io::Error::other("an output slot is already quarantined"));
        }
        let Some(mut frame) = self.ready.take() else {
            return Ok(false);
        };
        let slot = frame.slot;
        let timing_fence = frame.render_fence.take_timing_fd();
        self.quarantine_slot(
            slot,
            timing_fence,
            OutputQuarantineReason::SuspendAbandonment,
            Some(frame),
        )?;
        Ok(true)
    }

    pub(crate) fn suspend_abandon_worker_queued(
        &mut self,
        token: PageFlipToken,
    ) -> io::Result<bool> {
        if self.quarantine.is_some() {
            return Err(io::Error::other("an output slot is already quarantined"));
        }
        let Some(mut queued) = self.worker_queued.take() else {
            return Ok(false);
        };
        if queued.token != token {
            self.worker_queued = Some(queued);
            return Err(io::Error::other(
                "suspended worker output token does not match queued ownership",
            ));
        }
        let timing_fence = queued.frame.render_fence.take_timing_fd();
        self.quarantine_slot(
            queued.frame.slot,
            timing_fence,
            OutputQuarantineReason::SuspendAbandonment,
            Some(queued.frame),
        )?;
        Ok(true)
    }

    pub(crate) fn suspended_ready_fence_signaled(&self) -> io::Result<bool> {
        let Some(quarantine) = self.quarantine.as_ref() else {
            return Ok(true);
        };
        if quarantine.reason != OutputQuarantineReason::SuspendAbandonment {
            return Err(io::Error::other(
                "fatal output quarantine cannot recover to normal operation",
            ));
        }
        if quarantine.timing_fence.is_none() {
            return quarantine
                .abandoned_frame
                .as_ref()
                .map_or(Ok(false), |frame| {
                    frame.render_fence.is_signaled_nonblocking()
                });
        }
        let fence = quarantine.timing_fence.as_ref().expect("checked above");
        let mut pollfd = libc::pollfd {
            fd: fence.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut pollfd, 1, 0) };
        if ready < 0 {
            return Err(io::Error::last_os_error());
        }
        if pollfd.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err(io::Error::other(
                "suspended output render fence reported poll failure",
            ));
        }
        Ok(ready > 0 && pollfd.revents & libc::POLLIN != 0)
    }

    pub(crate) fn suspended_ready_fence_fd(&self) -> Option<RawFd> {
        self.quarantine
            .as_ref()
            .filter(|quarantine| quarantine.reason == OutputQuarantineReason::SuspendAbandonment)
            .and_then(|quarantine| {
                quarantine
                    .timing_fence
                    .as_ref()
                    .map(AsRawFd::as_raw_fd)
                    .or_else(|| {
                        quarantine
                            .abandoned_frame
                            .as_ref()
                            .and_then(|frame| frame.render_fence.readiness_fd())
                            .map(AsRawFd::as_raw_fd)
                    })
            })
    }

    pub(crate) fn has_suspended_ready_frame(&self) -> bool {
        self.quarantine.as_ref().is_some_and(|quarantine| {
            quarantine.reason == OutputQuarantineReason::SuspendAbandonment
                && quarantine.abandoned_frame.is_some()
        })
    }

    pub(crate) fn retire_pending_after_recovery(&mut self) -> Option<RenderedOutputFrame> {
        self.pending.take().map(|submitted| submitted.frame)
    }

    pub(crate) fn take_suspended_ready_frame(&mut self) -> Option<RenderedOutputFrame> {
        self.quarantine
            .as_mut()
            .and_then(|quarantine| quarantine.abandoned_frame.take())
    }

    pub(crate) fn rebind_pool_generation(&mut self, pool_generation: u64) -> io::Result<()> {
        if self.worker_queued.is_some()
            || self.pending.is_some()
            || self.ready.is_some()
            || self.rendering.is_some()
            || self.quarantine.is_some()
        {
            return Err(io::Error::other(
                "output pool generation cannot change while a non-current slot is owned",
            ));
        }
        self.pool_generation = pool_generation;
        self.last_presented_primary_claim = None;
        Ok(())
    }

    pub(crate) fn recover_suspended_slot(&mut self, fence_signaled: bool) -> io::Result<()> {
        let Some(quarantine) = self.quarantine.as_ref() else {
            return Ok(());
        };
        if quarantine.reason != OutputQuarantineReason::SuspendAbandonment {
            return Err(io::Error::other(
                "fatal output quarantine cannot recover to normal operation",
            ));
        }
        if !fence_signaled {
            return Err(io::Error::other(
                "suspended output slot render fence is not signaled",
            ));
        }
        if quarantine.abandoned_frame.is_some() {
            return Err(io::Error::other(
                "suspended ready frame release ownership has not been retired",
            ));
        }
        self.quarantine = None;
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
            && !is_strictly_later_claim(worker.frame.target.physical_claim(), claim)
        {
            return PhysicalPrimaryClaimRevalidation::OvertakesWorkerQueued {
                owner: QueuedOutputFrameIdentitySnapshot {
                    frame: (&worker.frame).into(),
                    token: worker.token,
                },
            };
        }
        if let Some(ready) = &self.ready
            && !is_strictly_later_claim(ready.target.physical_claim(), claim)
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
        self.pending.as_ref().map(|pending| pending.frame.target)
    }

    pub(crate) fn latest_future_primary_target(&self) -> Option<PresentationTarget> {
        self.worker_queued
            .as_ref()
            .map(|queued| queued.frame.target)
            .or_else(|| self.pending_target())
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
        self.quarantine
            .as_ref()
            .is_some_and(|quarantine| quarantine.reason.is_fatal())
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
            self.validate_target_claim(frame.target)?;
            if frame.pool_generation != self.pool_generation
                || frame.target.clock_generation != self.pool_generation
                || frame.target.physical_claim().clock_generation != self.pool_generation
            {
                return Err(io::Error::other(
                    "output frame belongs to an old swapchain generation",
                ));
            }
        }
        let frontier = PresentationOpportunityFrontier::from_claims(
            [
                self.pending.as_ref().map(|pending| pending.frame.target),
                self.worker_queued
                    .as_ref()
                    .map(|queued| queued.frame.target),
                self.ready.as_ref().map(|ready| ready.target),
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
        Ok(())
    }

    fn validate_worker_queued_frame(&self, frame: &RenderedOutputFrame) -> io::Result<()> {
        if self.ready.as_ref().is_some_and(|ready| {
            ready.slot != frame.slot
                || ready.id != frame.id
                || ready.transaction_id != frame.transaction_id
                || ready.pool_generation != frame.pool_generation
                || ready.target != frame.target
        }) {
            return Err(io::Error::other(
                "worker-queued frame does not match ready ownership",
            ));
        }
        if frame.pool_generation != self.pool_generation
            || frame.target.clock_generation != self.pool_generation
            || frame.target.physical_claim().clock_generation != self.pool_generation
            || frame.slot == self.current
            || self.pending_slot() == Some(frame.slot)
            || self.quarantine_slot_id() == Some(frame.slot)
            || self.rendering == Some(frame.slot)
        {
            return Err(io::Error::other(
                "worker-queued frame identity aliases another output owner",
            ));
        }
        self.validate_target_claim(frame.target)?;
        if let Some(last_presented) = self.last_presented_primary_claim {
            validate_strictly_later_claim(last_presented, frame.target.physical_claim())?;
        }
        if let Some(pending) = &self.pending {
            validate_strictly_later_target(pending.frame.target, frame.target)?;
        }
        Ok(())
    }

    fn validate_later_primary_target(&self, target: PresentationTarget) -> io::Result<()> {
        self.validate_target_claim(target)?;
        if let Some(last_presented) = self.last_presented_primary_claim {
            validate_strictly_later_claim(last_presented, target.physical_claim())?;
        }
        if let Some(worker) = &self.worker_queued {
            validate_strictly_later_target(worker.frame.target, target)
        } else if let Some(pending) = &self.pending {
            validate_strictly_later_target(pending.frame.target, target)
        } else {
            Ok(())
        }
    }

    fn validate_live_future_primary_claims(&self) -> io::Result<()> {
        if let Some(last_presented) = self.last_presented_primary_claim {
            if let Some(worker) = &self.worker_queued {
                validate_strictly_later_claim(
                    last_presented,
                    worker.frame.target.physical_claim(),
                )?;
            }
            if let Some(pending) = &self.pending {
                validate_strictly_later_claim(
                    last_presented,
                    pending.frame.target.physical_claim(),
                )?;
            }
            if let Some(ready) = &self.ready {
                validate_strictly_later_claim(last_presented, ready.target.physical_claim())?;
            }
        }
        if let (Some(pending), Some(worker)) = (&self.pending, &self.worker_queued) {
            validate_strictly_later_target(pending.frame.target, worker.frame.target)?;
        }
        if let Some(ready) = &self.ready {
            self.validate_later_primary_target(ready.target)?;
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
    }
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
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn test_render_fence() -> NativeRenderFence {
        let mut pipe = [-1; 2];
        assert_eq!(
            unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        unsafe { libc::close(pipe[1]) };
        NativeRenderFence::from_submission_fd(unsafe { OwnedFd::from_raw_fd(pipe[0]) })
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
            target,
            submit_window: KmsSubmitWindow::try_new(
                target.presentation_time.get(),
                target.submit_not_before().get(),
                0,
                0,
            )
            .expect("test submit window"),
            render_fence: test_render_fence(),
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
            cpu_prepass_duration_ns: 0,
            cpu_encode_duration_ns: 0,
            frozen_cursor_plan: FrozenPrimaryCursorPlan {
                delivery: PresentedCursorDelivery::Hidden,
                primary_presentation: FrozenPrimaryCursorPresentation::Preserve,
                cursor_test_policy: FrozenCursorTestPolicy::Skip,
            },
            frozen_cursor_plane_owner: None,
            o1_admission: None,
        }
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
