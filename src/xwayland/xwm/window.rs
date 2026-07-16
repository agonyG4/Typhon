use std::collections::HashMap;

use super::{X11WindowHandle, X11WindowSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11WindowLifecycle {
    Observed,
    ManagedPendingSurface,
    AssociatedPendingBuffer,
    Ready,
    Mapped,
    Withdrawn,
    Destroyed,
}

#[derive(Debug, Clone)]
pub(crate) struct X11WindowRecord {
    pub(crate) lifecycle: X11WindowLifecycle,
    pub(crate) snapshot: Option<X11WindowSnapshot>,
}

#[derive(Debug, Default)]
pub(crate) struct X11WindowRegistry {
    records: HashMap<X11WindowHandle, X11WindowRecord>,
}

impl X11WindowRegistry {
    pub(crate) fn insert_observed(&mut self, handle: X11WindowHandle) -> bool {
        if self.records.contains_key(&handle) {
            return false;
        }
        self.records.insert(
            handle,
            X11WindowRecord {
                lifecycle: X11WindowLifecycle::Observed,
                snapshot: None,
            },
        );
        true
    }

    pub(crate) fn insert_snapshot(&mut self, snapshot: X11WindowSnapshot) -> bool {
        if self.records.contains_key(&snapshot.handle) {
            return false;
        }
        self.records.insert(
            snapshot.handle,
            X11WindowRecord {
                lifecycle: X11WindowLifecycle::Ready,
                snapshot: Some(snapshot),
            },
        );
        true
    }

    pub(crate) fn get(&self, handle: X11WindowHandle) -> Option<&X11WindowRecord> {
        self.records.get(&handle)
    }

    pub(crate) fn get_mut(&mut self, handle: X11WindowHandle) -> Option<&mut X11WindowRecord> {
        self.records.get_mut(&handle)
    }

    pub(crate) fn remove(&mut self, handle: X11WindowHandle) -> Option<X11WindowRecord> {
        self.records.remove(&handle)
    }

    pub(crate) fn clear_generation(&mut self, generation: super::XwaylandGeneration) {
        self.records
            .retain(|handle, _| handle.generation() != generation);
    }

    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    pub(crate) fn contains(&self, handle: X11WindowHandle) -> bool {
        self.records.contains_key(&handle)
    }
}
