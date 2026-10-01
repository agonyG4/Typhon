use std::collections::VecDeque;

use wayland_server::backend::ClientId;

use super::protocol_error_trace::protocol_error_timestamp_ns;

const LIFECYCLE_COMPATIBILITY_TRACE_CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LifecycleCompatibilityViolation {
    SurfaceDestroyedWithLiveRole,
    XdgSurfaceDestroyedWithLiveRole,
    ActiveClipboardSourceReused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LifecycleCompatibilityAction {
    CanonicalSurfaceTeardown,
    CanonicalXdgRoleTeardown,
    KeepActiveClipboardSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LifecycleCompatibilityRecord {
    pub(crate) timestamp_ns: u64,
    pub(crate) client_id: ClientId,
    pub(crate) object_id: u32,
    pub(crate) interface: &'static str,
    pub(crate) surface_id: Option<u32>,
    pub(crate) violation: LifecycleCompatibilityViolation,
    pub(crate) action: LifecycleCompatibilityAction,
}

#[derive(Debug)]
pub(crate) struct LifecycleCompatibilityTrace {
    records: VecDeque<LifecycleCompatibilityRecord>,
}

impl LifecycleCompatibilityTrace {
    pub(crate) fn record(
        &mut self,
        client_id: ClientId,
        object_id: u32,
        interface: &'static str,
        surface_id: Option<u32>,
        violation: LifecycleCompatibilityViolation,
        action: LifecycleCompatibilityAction,
    ) {
        if self.records.len() == LIFECYCLE_COMPATIBILITY_TRACE_CAPACITY {
            self.records.pop_front();
        }
        self.records.push_back(LifecycleCompatibilityRecord {
            timestamp_ns: protocol_error_timestamp_ns(),
            client_id,
            object_id,
            interface,
            surface_id,
            violation,
            action,
        });
    }

    pub(crate) fn dump(&self) {
        for record in &self.records {
            eprintln!(
                "typhon_lifecycle_compatibility timestamp_ns={} client={:?} object_id={} interface={} surface_id={:?} violation={:?} action={:?}",
                record.timestamp_ns,
                record.client_id,
                record.object_id,
                record.interface,
                record.surface_id,
                record.violation,
                record.action,
            );
        }
    }
}

impl Default for LifecycleCompatibilityTrace {
    fn default() -> Self {
        Self {
            records: VecDeque::with_capacity(LIFECYCLE_COMPATIBILITY_TRACE_CAPACITY),
        }
    }
}
