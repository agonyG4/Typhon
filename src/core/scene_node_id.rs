use std::num::NonZeroU64;

/// Stable identity for one canonical visual scene object.
///
/// Values are allocated monotonically and are never reused during a
/// compositor session. The type owns no compositor state; protocol, window,
/// and output identities remain separate namespaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SceneNodeId(NonZeroU64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SceneNodeIdAllocationError {
    Exhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SceneNodeIdAllocator {
    next: NonZeroU64,
    exhausted: bool,
}

impl SceneNodeId {
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

impl Default for SceneNodeIdAllocator {
    fn default() -> Self {
        Self {
            next: NonZeroU64::MIN,
            exhausted: false,
        }
    }
}

impl SceneNodeIdAllocator {
    pub(crate) fn allocate(&mut self) -> Result<SceneNodeId, SceneNodeIdAllocationError> {
        if self.exhausted {
            return Err(SceneNodeIdAllocationError::Exhausted);
        }
        let id = SceneNodeId::new(self.next);
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
use std::collections::{HashSet, hash_map::DefaultHasher};
#[cfg(test)]
use std::hash::{Hash, Hasher};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_cannot_create_a_scene_node_id() {
        assert_eq!(SceneNodeId::from_raw(0), None);
    }

    #[test]
    fn allocator_is_monotonic_and_does_not_recycle() {
        let mut allocator = SceneNodeIdAllocator::default();
        let first = allocator.allocate().expect("first scene node id");
        let second = allocator.allocate().expect("second scene node id");

        assert_eq!(first.get(), 1);
        assert_eq!(second.get(), 2);
        assert!(first < second);
        assert_ne!(first, second);
    }

    #[test]
    fn scene_node_ids_hash_and_order_as_typed_scalars() {
        let first = SceneNodeId::from_raw(7).expect("nonzero scene node id");
        let second = SceneNodeId::from_raw(8).expect("nonzero scene node id");
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
    fn allocator_rejects_exhaustion_without_recycling_or_wrapping() {
        let mut allocator = SceneNodeIdAllocator::with_next(NonZeroU64::new(u64::MAX).unwrap());
        let last = allocator.allocate().expect("last scene node id");

        assert_eq!(last.get(), u64::MAX);
        assert_eq!(
            allocator.allocate(),
            Err(SceneNodeIdAllocationError::Exhausted)
        );
    }

    #[test]
    fn scene_node_namespace_is_not_constructed_from_other_identity_types() {
        let scene_node = SceneNodeId::from_raw(17).expect("scene node id");
        let output = crate::core::OutputId::from_raw(17).expect("output id");
        let window = crate::core::WindowId::from_raw(17).expect("window id");

        assert_eq!(scene_node.get(), output.get());
        assert_eq!(scene_node.get(), window.get());
        assert_ne!(format!("{scene_node:?}"), format!("{output:?}"));
        assert_ne!(format!("{scene_node:?}"), format!("{window:?}"));
    }
}
