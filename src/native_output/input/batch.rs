use super::*;

pub(crate) const NATIVE_INPUT_DRAIN_BUDGET: usize = 256;

#[derive(Debug, Default)]
pub(crate) struct NativeInputBatch {
    pub(crate) raw: Vec<NativeHardwareInputEvent>,
    pub(crate) coalesced: Vec<NativeHardwareInputEvent>,
    /// The bounded drain stopped before the backend reported exhaustion.
    pub(crate) budget_exhausted: bool,
}

impl NativeInputBatch {
    pub(crate) fn coalesce_pointer_motion_events(&mut self) {
        self.coalesced.clear();
        let mut pending_motion = None;
        for event in self.raw.drain(..) {
            match event {
                NativeHardwareInputEvent::PointerMotion(sample) => match pending_motion {
                    Some(PendingPointerMotion::Sample(pending_sample)) => {
                        if let Some(coalesced_sample) = pending_sample.coalesce(sample) {
                            pending_motion = Some(PendingPointerMotion::Sample(coalesced_sample));
                        } else {
                            flush_pending_pointer_motion(
                                &mut self.coalesced,
                                PendingPointerMotion::Sample(pending_sample),
                            );
                            pending_motion = Some(PendingPointerMotion::Sample(sample));
                        }
                    }
                    None => pending_motion = Some(PendingPointerMotion::Sample(sample)),
                },
                event => {
                    if let Some(pending) = pending_motion.take() {
                        flush_pending_pointer_motion(&mut self.coalesced, pending);
                    }
                    self.coalesced.push(event);
                }
            }
        }
        if let Some(pending) = pending_motion {
            flush_pending_pointer_motion(&mut self.coalesced, pending);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warm_batches_retain_raw_and_coalesced_capacity() {
        let mut batch = NativeInputBatch::default();
        batch.raw.reserve(8);
        batch.coalesced.reserve(8);
        let raw_capacity = batch.raw.capacity();
        let coalesced_capacity = batch.coalesced.capacity();
        batch.raw.clear();
        batch.coalesced.clear();
        assert_eq!(batch.raw.capacity(), raw_capacity);
        assert_eq!(batch.coalesced.capacity(), coalesced_capacity);
    }
}
