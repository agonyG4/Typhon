use std::num::NonZeroU64;

#[cfg(test)]
use std::collections::{HashSet, hash_map::DefaultHasher};
#[cfg(test)]
use std::hash::{Hash, Hasher};

/// Stable identity for one logical compositor output.
///
/// Values are allocated monotonically by the compositor and are never reused
/// during a compositor session. The type itself owns no output or lifecycle
/// state; backend generations remain separate identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutputId(NonZeroU64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputIdAllocationError {
    Exhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputIdAllocator {
    next: NonZeroU64,
    exhausted: bool,
}

impl OutputId {
    pub(crate) const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    pub const fn from_raw(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }

    pub const fn raw(self) -> u64 {
        self.get()
    }
}

impl Default for OutputIdAllocator {
    fn default() -> Self {
        Self {
            next: NonZeroU64::MIN,
            exhausted: false,
        }
    }
}

impl OutputIdAllocator {
    pub(crate) fn allocate(&mut self) -> Result<OutputId, OutputIdAllocationError> {
        if self.exhausted {
            return Err(OutputIdAllocationError::Exhausted);
        }
        let id = OutputId::new(self.next);
        if id.get() == u64::MAX {
            self.exhausted = true;
        } else {
            self.next = NonZeroU64::new(id.get() + 1).expect("increment remains nonzero");
        }
        Ok(id)
    }

    #[cfg(test)]
    const fn with_next(next: NonZeroU64) -> Self {
        Self {
            next,
            exhausted: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_cannot_create_an_output_id() {
        assert_eq!(OutputId::from_raw(0), None);
    }

    #[test]
    fn allocator_is_monotonic_and_does_not_recycle() {
        let mut allocator = OutputIdAllocator::default();
        let first = allocator.allocate().expect("first output id");
        let second = allocator.allocate().expect("second output id");

        assert_eq!(first.get(), 1);
        assert_eq!(second.get(), 2);
        assert!(first < second);
        assert_ne!(first, second);
    }

    #[test]
    fn output_ids_hash_and_order_as_typed_scalars() {
        let first = OutputId::from_raw(7).expect("nonzero output id");
        let second = OutputId::from_raw(8).expect("nonzero output id");
        let mut ids = HashSet::new();
        ids.insert(first);
        ids.insert(second);

        let mut first_hasher = DefaultHasher::new();
        first.hash(&mut first_hasher);
        let mut second_hasher = DefaultHasher::new();
        second.hash(&mut second_hasher);

        assert_eq!(ids.len(), 2);
        assert_ne!(first_hasher.finish(), second_hasher.finish());
        assert!(first < second);
    }

    #[test]
    fn allocator_rejects_exhaustion_without_recycling() {
        let mut allocator = OutputIdAllocator::with_next(NonZeroU64::new(u64::MAX).unwrap());
        let last = allocator.allocate().expect("last output id");

        assert_eq!(last.get(), u64::MAX);
        assert_eq!(
            allocator.allocate(),
            Err(OutputIdAllocationError::Exhausted)
        );
    }
}
