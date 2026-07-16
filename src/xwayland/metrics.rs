#[derive(Debug, Default)]
pub(crate) struct XwaylandMetrics {
    pub(crate) state_transitions: u64,
    pub(crate) generations_started: u64,
    pub(crate) stale_events: u64,
    pub(crate) readiness_failures: u64,
}
