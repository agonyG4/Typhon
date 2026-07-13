use std::io;

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
