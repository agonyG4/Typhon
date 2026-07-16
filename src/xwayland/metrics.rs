#[derive(Debug, Default)]
pub(crate) struct XwaylandMetrics {
    pub(crate) state_transitions: u64,
    pub(crate) generations_started: u64,
    pub(crate) lazy_triggers: u64,
    pub(crate) startup_duration_ns: Option<u64>,
    pub(crate) stale_events: u64,
    pub(crate) readiness_failures: u64,
    pub(crate) crashes: u64,
    pub(crate) backoff_level: usize,
    pub(crate) unauthorized_bind_attempts: u64,
    pub(crate) association_commits: u64,
    pub(crate) association_removals: u64,
    pub(crate) cleanup_attempts: u64,
    pub(crate) cleanup_failures: u64,
    pub(crate) xwm_events_received: u64,
    pub(crate) xwm_drain_budget_exhaustions: u64,
    pub(crate) xwm_connection_failures: u64,
}
