use std::{
    io,
    os::fd::{AsRawFd, OwnedFd, RawFd},
};

#[cfg(test)]
use std::num::NonZeroU64;

use oblivion_one::compositor::{CompositorFrameBatchId, SurfaceDamagePresentation};
use oblivion_one::native::kms::PageFlipToken;
#[cfg(test)]
use oblivion_one::native::presentation_deadline::PresentationTargetReason;
use oblivion_one::native::presentation_deadline::{MonotonicTimestampNs, PresentationTarget};
use oblivion_one::native::scheduler::NativeOutputPacingMode;

use crate::egl_renderer::{EglSceneFrameCommit, native_fence::NativeRenderFence};
use crate::native_output::OutputTransactionId;

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
    pub(crate) render_generation: u64,
    pub(crate) pool_generation: u64,
    pub(crate) target: PresentationTarget,
    pub(crate) render_fence: NativeRenderFence,
    pub(crate) scene_commit: EglSceneFrameCommit,
    pub(crate) surface_damage: SurfaceDamagePresentation,
    pub(crate) protocol_batch_id: CompositorFrameBatchId,
    pub(crate) composite_started_at: MonotonicTimestampNs,
    pub(crate) fence_exported_at: MonotonicTimestampNs,
    pub(crate) rendered_at: MonotonicTimestampNs,
    pub(crate) cpu_prepass_duration_ns: u64,
    pub(crate) cpu_encode_duration_ns: u64,
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
    pending: Option<SubmittedOutputFrame>,
    ready: Option<RenderedOutputFrame>,
    rendering: Option<OutputSlotId>,
    quarantine: Option<QuarantinedOutputSlot>,
    next_frame_id: u64,
    presentation_serial: u64,
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
            pending: None,
            ready: None,
            rendering: None,
            quarantine: None,
            next_frame_id: 1,
            presentation_serial: 0,
        })
    }

    pub(crate) fn acquire_render_slot(&mut self) -> io::Result<OutputSlotId> {
        self.acquire_render_slot_for(NativeOutputPacingMode::PredictiveTriple)
    }

    pub(crate) fn acquire_render_slot_for(
        &mut self,
        pacing_mode: NativeOutputPacingMode,
    ) -> io::Result<OutputSlotId> {
        self.ensure_operational()?;
        if self.rendering.is_some() {
            return Err(io::Error::other("an output slot is already rendering"));
        }
        if self.ready.is_some() {
            return Err(io::Error::other("an output frame is already ready"));
        }
        if pacing_mode == NativeOutputPacingMode::ReactiveDouble && self.pending.is_some() {
            return Err(io::Error::other(
                "ReactiveDouble cannot acquire a third output slot while pageflip is pending",
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
        !self.is_poisoned()
            && self.rendering.is_none()
            && self.ready.is_none()
            && !(pacing_mode == NativeOutputPacingMode::ReactiveDouble && self.pending.is_some())
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
        let now = MonotonicTimestampNs::new(0);
        let target = PresentationTarget {
            sequence: 1,
            presentation_time: now,
            submit_not_before: now,
            render_start_deadline: now,
            refresh_interval: std::time::Duration::from_nanos(1),
            reason: PresentationTargetReason::ForcedValidation,
            clock_generation: self.pool_generation,
            estimated: true,
            predicted_unreachable: false,
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
        let surface_damage = server.capture_surface_damage_presentation();
        self.finish_render_owned(RenderedOutputFrame {
            id: self.next_frame_id,
            transaction_id: OutputTransactionId::new(
                NonZeroU64::new(self.next_frame_id).expect("test transaction ID is nonzero"),
            ),
            slot,
            render_generation,
            pool_generation: self.pool_generation,
            target,
            render_fence,
            scene_commit: EglSceneFrameCommit::empty_for_test(),
            surface_damage,
            protocol_batch_id,
            composite_started_at: now,
            fence_exported_at: now,
            rendered_at: now,
            cpu_prepass_duration_ns: 0,
            cpu_encode_duration_ns: 0,
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

    pub(crate) fn take_ready_for_submission(&mut self) -> io::Result<RenderedOutputFrame> {
        self.ensure_operational()?;
        if self.pending.is_some() {
            return Err(io::Error::other("an output pageflip is already pending"));
        }
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
        if self.pending.is_some() || frame.pool_generation != self.pool_generation {
            return Err(io::Error::other(
                "submitted output frame does not match available pending ownership",
            ));
        }
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

    pub(crate) fn suspended_ready_fence_signaled(&self) -> io::Result<bool> {
        let Some(quarantine) = self.quarantine.as_ref() else {
            return Ok(true);
        };
        if quarantine.reason != OutputQuarantineReason::SuspendAbandonment {
            return Err(io::Error::other(
                "fatal output quarantine cannot recover to normal operation",
            ));
        }
        let Some(fence) = quarantine.timing_fence.as_ref() else {
            return Ok(false);
        };
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

    pub(crate) fn retire_pending_after_recovery(&mut self) -> Option<RenderedOutputFrame> {
        self.pending.take().map(|submitted| submitted.frame)
    }

    pub(crate) fn take_suspended_ready_frame(&mut self) -> Option<RenderedOutputFrame> {
        self.quarantine
            .as_mut()
            .and_then(|quarantine| quarantine.abandoned_frame.take())
    }

    pub(crate) fn rebind_pool_generation(&mut self, pool_generation: u64) -> io::Result<()> {
        if self.pending.is_some()
            || self.ready.is_some()
            || self.rendering.is_some()
            || self.quarantine.is_some()
        {
            return Err(io::Error::other(
                "output pool generation cannot change while a non-current slot is owned",
            ));
        }
        self.pool_generation = pool_generation;
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
        self.ensure_operational()?;
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

    pub(crate) const fn current(&self) -> OutputSlotId {
        self.current
    }

    pub(crate) const fn presentation_serial(&self) -> u64 {
        self.presentation_serial
    }

    pub(crate) fn pending_slot(&self) -> Option<OutputSlotId> {
        self.pending.as_ref().map(|pending| pending.frame.slot)
    }

    pub(crate) fn pending_token(&self) -> Option<PageFlipToken> {
        self.pending.as_ref().map(|pending| pending.token)
    }

    pub(crate) fn pending_target(&self) -> Option<PresentationTarget> {
        self.pending.as_ref().map(|pending| pending.frame.target)
    }

    pub(crate) fn pending_frame_mut(&mut self) -> Option<&mut RenderedOutputFrame> {
        self.pending.as_mut().map(|pending| &mut pending.frame)
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
        Ok(())
    }

    pub(crate) fn validate_invariants_for(
        &self,
        pacing_mode: NativeOutputPacingMode,
    ) -> io::Result<()> {
        self.validate_invariants()?;
        if pacing_mode == NativeOutputPacingMode::ReactiveDouble
            && self.pending.is_some()
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

    fn ensure_operational(&self) -> io::Result<()> {
        if self.quarantine.is_some() {
            return Err(io::Error::other(
                "explicit output swapchain is quarantined and non-renderable",
            ));
        }
        Ok(())
    }

    fn slot_is_free(&self, slot: OutputSlotId) -> bool {
        slot != self.current
            && self.pending_slot() != Some(slot)
            && self.ready_slot() != Some(slot)
            && self.rendering != Some(slot)
            && self.quarantine_slot_id() != Some(slot)
    }
}
