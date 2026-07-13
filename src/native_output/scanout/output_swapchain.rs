use std::{io, os::fd::OwnedFd};

use oblivion_one::native::kms::PageFlipToken;

use crate::egl_renderer::native_fence::NativeRenderFence;

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
}

#[derive(Debug)]
pub(crate) struct RenderedOutputFrame {
    pub(crate) id: u64,
    pub(crate) slot: OutputSlotId,
    pub(crate) render_generation: u64,
    pub(crate) pool_generation: u64,
    pub(crate) render_fence: NativeRenderFence,
}

#[derive(Debug)]
pub(crate) struct SubmittedOutputFrame {
    pub(crate) frame: RenderedOutputFrame,
    pub(crate) token: PageFlipToken,
    pub(crate) out_fence: Option<OwnedFd>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompletedOutputFrame {
    pub(crate) frame_id: u64,
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
        self.ensure_operational()?;
        if self.rendering.is_some() {
            return Err(io::Error::other("an output slot is already rendering"));
        }
        if self.ready.is_some() {
            return Err(io::Error::other("an output frame is already ready"));
        }
        let slot = self
            .slots
            .iter()
            .find(|slot| self.slot_is_free(*slot))
            .ok_or_else(|| io::Error::other("no explicit output slot is free"))?;
        self.rendering = Some(slot);
        Ok(slot)
    }

    pub(crate) fn finish_render(
        &mut self,
        slot: OutputSlotId,
        render_generation: u64,
        render_fence: NativeRenderFence,
    ) -> io::Result<u64> {
        self.ensure_operational()?;
        if self.rendering != Some(slot) {
            return Err(io::Error::other(
                "finished output slot does not match active rendering ownership",
            ));
        }
        if self.ready.is_some() {
            return Err(io::Error::other("an output frame is already ready"));
        }
        let frame_id = self.next_frame_id;
        let next_frame_id = self
            .next_frame_id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("output frame ID overflow"))?;
        self.next_frame_id = next_frame_id;
        self.rendering = None;
        self.ready = Some(RenderedOutputFrame {
            id: frame_id,
            slot,
            render_generation,
            pool_generation: self.pool_generation,
            render_fence,
        });
        Ok(frame_id)
    }

    pub(crate) fn submit_ready(
        &mut self,
        token: PageFlipToken,
        out_fence: Option<OwnedFd>,
    ) -> io::Result<()> {
        self.ensure_operational()?;
        if self.pending.is_some() {
            return Err(io::Error::other("an output pageflip is already pending"));
        }
        let frame = self
            .ready
            .take()
            .ok_or_else(|| io::Error::other("no rendered output frame is ready"))?;
        self.pending = Some(SubmittedOutputFrame {
            frame,
            token,
            out_fence,
        });
        Ok(())
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
        self.quarantine_slot(slot, timing_fence, reason)?;
        Ok(slot)
    }

    pub(crate) fn suspend_abandon_ready(&mut self) -> io::Result<Option<OutputSlotId>> {
        if self.quarantine.is_some() {
            return Err(io::Error::other("an output slot is already quarantined"));
        }
        let Some(mut frame) = self.ready.take() else {
            return Ok(None);
        };
        let slot = frame.slot;
        let timing_fence = frame.render_fence.take_timing_fd();
        self.quarantine_slot(
            slot,
            timing_fence,
            OutputQuarantineReason::SuspendAbandonment,
        )?;
        Ok(Some(slot))
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
            frame_id: pending.frame.id,
            old_current,
            new_current: self.current,
            presentation_serial: self.presentation_serial,
        })
    }

    pub(crate) const fn current(&self) -> OutputSlotId {
        self.current
    }

    pub(crate) fn pending_slot(&self) -> Option<OutputSlotId> {
        self.pending.as_ref().map(|pending| pending.frame.slot)
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

    fn quarantine_slot(
        &mut self,
        slot: OutputSlotId,
        timing_fence: Option<OwnedFd>,
        reason: OutputQuarantineReason,
    ) -> io::Result<()> {
        if self.quarantine.is_some() {
            return Err(io::Error::other("an output slot is already quarantined"));
        }
        self.quarantine = Some(QuarantinedOutputSlot {
            slot,
            pool_generation: self.pool_generation,
            timing_fence,
            reason,
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
