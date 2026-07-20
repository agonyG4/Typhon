//! Generation-bound adoption and readiness deadlines.

use std::collections::{HashMap, VecDeque};

use super::X11WindowHandle;

pub(crate) const ADOPTION_TIMEOUT_NS: u64 = 5_000_000_000;

pub(crate) fn take_batch(queue: &mut VecDeque<u32>, capacity: usize) -> Vec<u32> {
    (0..capacity).filter_map(|_| queue.pop_front()).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdoptionWait {
    MapToAssociation,
    AssociationToBuffer,
    SerialPair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingAdoption {
    generation: super::XwaylandGeneration,
    deadline_ns: u64,
    wait: AdoptionWait,
}

#[derive(Debug, Default)]
pub(crate) struct AdoptionTracker {
    pending: HashMap<X11WindowHandle, PendingAdoption>,
}

impl AdoptionTracker {
    pub(crate) fn observe(
        &mut self,
        handle: X11WindowHandle,
        wait: AdoptionWait,
        deadline_ns: u64,
    ) {
        self.pending.insert(
            handle,
            PendingAdoption {
                generation: handle.generation(),
                deadline_ns,
                wait,
            },
        );
    }

    pub(crate) fn complete(&mut self, handle: X11WindowHandle) {
        self.pending.remove(&handle);
    }

    pub(crate) fn clear_generation(&mut self, generation: super::XwaylandGeneration) {
        self.pending.retain(|handle, pending| {
            handle.generation() != generation && pending.generation != generation
        });
    }

    pub(crate) fn expired(&mut self, now_ns: u64) -> Vec<(X11WindowHandle, AdoptionWait)> {
        let expired = self
            .pending
            .iter()
            .filter_map(|(handle, pending)| {
                (now_ns >= pending.deadline_ns).then_some((*handle, pending.wait))
            })
            .collect::<Vec<_>>();
        for (handle, _) in &expired {
            self.pending.remove(handle);
        }
        expired
    }

    pub(crate) fn next_deadline_ns(&self) -> Option<u64> {
        self.pending
            .values()
            .map(|pending| pending.deadline_ns)
            .min()
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use super::*;

    #[test]
    fn adoption_deadlines_are_generation_bound_and_bounded() {
        let generation = super::super::XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
        let handle = X11WindowHandle::new(generation, 22);
        let mut tracker = AdoptionTracker::default();
        tracker.observe(handle, AdoptionWait::MapToAssociation, 10);
        assert!(tracker.expired(9).is_empty());
        assert_eq!(
            tracker.expired(10),
            [(handle, AdoptionWait::MapToAssociation)]
        );
    }

    #[test]
    fn query_tree_over_capacity_is_processed_in_batches() {
        let mut queue = (1..=257).collect::<VecDeque<_>>();
        let mut seen = Vec::new();
        while !queue.is_empty() {
            seen.extend(take_batch(&mut queue, 64));
        }
        assert_eq!(seen.len(), 257);
        assert_eq!(seen.first(), Some(&1));
        assert_eq!(seen.last(), Some(&257));
    }
}
