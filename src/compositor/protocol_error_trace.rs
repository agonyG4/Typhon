#[cfg(test)]
mod tests {
    use super::*;
    use wayland_protocols::wp::pointer_constraints::zv1::server::zwp_pointer_constraints_v1::ZwpPointerConstraintsV1;

    fn record(
        interface: ProtocolErrorInterface,
        category: ProtocolErrorCategory,
    ) -> ProtocolErrorRecord {
        ProtocolErrorRecord {
            timestamp_ns: 10,
            client_id: None,
            peer_pid: None,
            interface,
            resource_id: Some(4),
            error_code: Some(7),
            surface_id: Some(22),
            xwayland_generation: Some(3),
            category,
        }
    }

    #[test]
    fn protocol_error_ring_is_bounded_and_keeps_newest_records() {
        let mut trace = ProtocolErrorTrace::new(true, 2);
        trace.record(record(
            ProtocolErrorInterface::CoreSurface,
            ProtocolErrorCategory::Wire,
        ));
        trace.record(record(
            ProtocolErrorInterface::Fifo,
            ProtocolErrorCategory::SurfaceDestroyed,
        ));
        trace.record(record(
            ProtocolErrorInterface::CommitTiming,
            ProtocolErrorCategory::SurfaceDestroyed,
        ));

        let records = trace.records().collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].interface, ProtocolErrorInterface::Fifo);
        assert_eq!(records[1].interface, ProtocolErrorInterface::CommitTiming);
        assert_eq!(records[1].error_code, Some(7));
    }

    #[test]
    fn required_protocol_error_categories_are_distinguishable() {
        let mut trace = ProtocolErrorTrace::new(true, 8);
        for (interface, category) in [
            (
                ProtocolErrorInterface::XwaylandShell,
                ProtocolErrorCategory::Wire,
            ),
            (ProtocolErrorInterface::Syncobj, ProtocolErrorCategory::Wire),
            (
                ProtocolErrorInterface::Fifo,
                ProtocolErrorCategory::SurfaceDestroyed,
            ),
            (
                ProtocolErrorInterface::CommitTiming,
                ProtocolErrorCategory::SurfaceDestroyed,
            ),
            (
                ProtocolErrorInterface::CoreSurface,
                ProtocolErrorCategory::Wire,
            ),
        ] {
            trace.record(record(interface, category));
        }

        let records = trace.records().collect::<Vec<_>>();
        assert_eq!(records.len(), 5);
        assert_eq!(records[0].interface, ProtocolErrorInterface::XwaylandShell);
        assert_eq!(records[1].interface, ProtocolErrorInterface::Syncobj);
        assert_eq!(records[2].category, ProtocolErrorCategory::SurfaceDestroyed);
        assert_eq!(records[3].category, ProtocolErrorCategory::SurfaceDestroyed);
        assert_eq!(records[4].interface, ProtocolErrorInterface::CoreSurface);
    }

    #[test]
    fn disabled_protocol_error_trace_retains_no_records() {
        let mut trace = ProtocolErrorTrace::new(false, 8);
        trace.record(record(
            ProtocolErrorInterface::CoreSurface,
            ProtocolErrorCategory::Unavailable,
        ));
        assert_eq!(trace.len(), 0);
    }

    #[test]
    fn pointer_constraints_manager_errors_are_classified_explicitly() {
        assert_eq!(
            ProtocolErrorInterface::for_resource::<ZwpPointerConstraintsV1>(),
            ProtocolErrorInterface::PointerConstraints
        );
    }
}
use std::{any::type_name, collections::VecDeque, sync::OnceLock, time::Instant};

use wayland_server::{Resource, backend::ClientId};

pub(crate) const PROTOCOL_ERROR_TRACE_CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProtocolErrorInterface {
    CoreSurface,
    PointerConstraints,
    XdgShell,
    XwaylandShell,
    Syncobj,
    Fifo,
    CommitTiming,
    LinuxDmabuf,
    WlDrm,
    Other,
}

impl ProtocolErrorInterface {
    pub(crate) fn for_resource<I: Resource>() -> Self {
        let name = type_name::<I>();
        if name.contains("WlSurface") {
            Self::CoreSurface
        } else if name.contains("PointerConstraints") {
            Self::PointerConstraints
        } else if name.contains("Xwayland") {
            Self::XwaylandShell
        } else if name.contains("Syncobj") || name.contains("DrmSyncobj") {
            Self::Syncobj
        } else if name.contains("Fifo") {
            Self::Fifo
        } else if name.contains("CommitTimer") || name.contains("CommitTiming") {
            Self::CommitTiming
        } else if name.contains("LinuxBuffer") || name.contains("Dmabuf") {
            Self::LinuxDmabuf
        } else if name.contains("WlDrm") {
            Self::WlDrm
        } else if name.contains("Xdg") {
            Self::XdgShell
        } else {
            Self::Other
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProtocolErrorCategory {
    Wire,
    SurfaceDestroyed,
    InvalidState,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProtocolErrorRecord {
    pub(crate) timestamp_ns: u64,
    pub(crate) client_id: Option<ClientId>,
    pub(crate) peer_pid: Option<i32>,
    pub(crate) interface: ProtocolErrorInterface,
    pub(crate) resource_id: Option<u32>,
    pub(crate) error_code: Option<u32>,
    pub(crate) surface_id: Option<u32>,
    pub(crate) xwayland_generation: Option<u64>,
    pub(crate) category: ProtocolErrorCategory,
}

#[derive(Debug)]
pub(crate) struct ProtocolErrorTrace {
    records: Option<VecDeque<ProtocolErrorRecord>>,
    capacity: usize,
}

impl ProtocolErrorTrace {
    pub(crate) fn new(enabled: bool, capacity: usize) -> Self {
        Self {
            records: enabled.then(|| VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    pub(crate) fn record(&mut self, record: ProtocolErrorRecord) {
        let Some(records) = self.records.as_mut() else {
            return;
        };
        if self.capacity == 0 {
            return;
        }
        if records.len() == self.capacity {
            records.pop_front();
        }
        records.push_back(record);
    }

    pub(crate) fn record_for_resource<I: Resource>(
        &mut self,
        resource: &I,
        error_code: u32,
        category: ProtocolErrorCategory,
    ) {
        self.record(ProtocolErrorRecord {
            timestamp_ns: protocol_error_timestamp_ns(),
            client_id: resource.client().map(|client| client.id()),
            peer_pid: None,
            interface: ProtocolErrorInterface::for_resource::<I>(),
            resource_id: Some(resource.id().protocol_id()),
            error_code: Some(error_code),
            surface_id: None,
            xwayland_generation: None,
            category,
        });
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.records.as_ref().map_or(0, VecDeque::len)
    }

    pub(crate) fn records(&self) -> impl Iterator<Item = &ProtocolErrorRecord> {
        self.records.iter().flat_map(|records| records.iter())
    }

    pub(crate) fn dump(&self) {
        for record in self.records() {
            eprintln!(
                "typhon_protocol_error timestamp_ns={} client={:?} pid={:?} interface={:?} resource_id={:?} error_code={:?} surface_id={:?} xwayland_generation={:?} category={:?}",
                record.timestamp_ns,
                record.client_id,
                record.peer_pid,
                record.interface,
                record.resource_id,
                record.error_code,
                record.surface_id,
                record.xwayland_generation,
                record.category,
            );
        }
    }
}

impl Default for ProtocolErrorTrace {
    fn default() -> Self {
        Self::new(true, PROTOCOL_ERROR_TRACE_CAPACITY)
    }
}

pub(crate) fn protocol_error_timestamp_ns() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
        .try_into()
        .unwrap_or(u64::MAX)
}
