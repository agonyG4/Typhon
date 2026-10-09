use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const REACTOR_RR_PRIORITY: i32 = 1;
const HIGH_PRIORITY_NICE: i32 = -10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchedulingQosPolicy {
    Off,
    Auto,
    Require,
}

impl SchedulingQosPolicy {
    pub(crate) fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("off") {
            "off" => Ok(Self::Off),
            "auto" => Ok(Self::Auto),
            "require" => Ok(Self::Require),
            other => Err(format!("invalid scheduling QoS policy `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SchedulingThreadRole {
    NativeReactor,
    KmsCommit,
    Render,
    Background,
}

impl SchedulingThreadRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NativeReactor => "native_reactor",
            Self::KmsCommit => "kms_commit",
            Self::Render => "render",
            Self::Background => "background",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulingClass {
    RealtimeRoundRobin { priority: i32 },
    HighPriorityFair { nice: i32 },
    Normal,
    Background { nice: i32 },
}

impl SchedulingClass {
    fn requested_for(role: SchedulingThreadRole, baseline_nice: i32) -> Self {
        match role {
            SchedulingThreadRole::NativeReactor => Self::RealtimeRoundRobin {
                priority: REACTOR_RR_PRIORITY,
            },
            SchedulingThreadRole::KmsCommit | SchedulingThreadRole::Render => {
                Self::HighPriorityFair {
                    nice: HIGH_PRIORITY_NICE,
                }
            }
            SchedulingThreadRole::Background => Self::Background {
                nice: baseline_nice,
            },
        }
    }

    fn requested_priority(self) -> Option<i32> {
        match self {
            Self::RealtimeRoundRobin { priority } => Some(priority),
            Self::HighPriorityFair { .. } | Self::Normal | Self::Background { .. } => None,
        }
    }

    fn requested_nice(self) -> Option<i32> {
        match self {
            Self::HighPriorityFair { nice } | Self::Background { nice } => Some(nice),
            Self::RealtimeRoundRobin { .. } | Self::Normal => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RealtimeRoundRobin { .. } => "sched_rr",
            Self::HighPriorityFair { .. } => "sched_other_high_priority",
            Self::Normal => "sched_other_normal",
            Self::Background { .. } => "sched_other_background",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectiveSchedulingClass {
    RealtimeRoundRobin {
        priority: i32,
    },
    RealtimeFifo {
        priority: i32,
    },
    Deadline,
    Fair {
        nice: i32,
    },
    Unknown {
        policy: i32,
        priority: i32,
        nice: i32,
    },
}

impl EffectiveSchedulingClass {
    fn from_kernel(state: KernelSchedulingState) -> Self {
        match state.policy {
            PolicyKind::RoundRobin => Self::RealtimeRoundRobin {
                priority: state.priority,
            },
            PolicyKind::Fifo => Self::RealtimeFifo {
                priority: state.priority,
            },
            PolicyKind::Deadline => Self::Deadline,
            PolicyKind::Other => Self::Fair { nice: state.nice },
            PolicyKind::Unknown(policy) => Self::Unknown {
                policy,
                priority: state.priority,
                nice: state.nice,
            },
        }
    }

    fn is_expected_reactor_rr(self, reset_on_fork: bool) -> bool {
        matches!(
            self,
            Self::RealtimeRoundRobin {
                priority: REACTOR_RR_PRIORITY
            }
        ) && reset_on_fork
    }

    fn is_fair_priority_above(self, baseline_nice: i32) -> bool {
        matches!(self, Self::Fair { nice } if nice < baseline_nice)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RealtimeRoundRobin { .. } => "sched_rr",
            Self::RealtimeFifo { .. } => "sched_fifo",
            Self::Deadline => "sched_deadline",
            Self::Fair { .. } => "sched_other",
            Self::Unknown { .. } => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulingBackend {
    Direct,
    RealtimeKit,
    Inherited,
    None,
}

impl SchedulingBackend {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::RealtimeKit => "rtkit",
            Self::Inherited => "inherited",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolicyKind {
    RoundRobin,
    Fifo,
    Deadline,
    Other,
    Unknown(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KernelSchedulingState {
    policy: PolicyKind,
    priority: i32,
    nice: i32,
    reset_on_fork: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulingDiagnosticSnapshot {
    pub role: SchedulingThreadRole,
    pub tid: i32,
    pub requested_class: SchedulingClass,
    pub effective_class: EffectiveSchedulingClass,
    pub backend: SchedulingBackend,
    pub requested_priority: Option<i32>,
    pub requested_nice: Option<i32>,
    pub requested_fallback_nice: Option<i32>,
    pub effective_priority: i32,
    pub effective_nice: i32,
    pub reset_on_fork_effective: bool,
    pub demotion_count: u64,
    pub rt_budget_overrun: bool,
    pub degradation_reason: Option<String>,
}

impl SchedulingDiagnosticSnapshot {
    pub fn detail(&self) -> String {
        format!(
            "role={} tid={} requested_class={} requested_priority={} requested_nice={} requested_fallback_nice={} effective_class={} effective_priority={} effective_nice={} backend={} reset_on_fork_effective={} demotion_count={} rt_budget_overrun={} degradation_reason={}",
            self.role.as_str(),
            self.tid,
            self.requested_class.as_str(),
            self.requested_priority
                .map_or_else(|| "none".to_string(), |value| value.to_string()),
            self.requested_nice
                .map_or_else(|| "none".to_string(), |value| value.to_string()),
            self.requested_fallback_nice
                .map_or_else(|| "none".to_string(), |value| value.to_string()),
            self.effective_class.as_str(),
            self.effective_priority,
            self.effective_nice,
            self.backend.as_str(),
            self.reset_on_fork_effective,
            self.demotion_count,
            self.rt_budget_overrun,
            self.degradation_reason.as_deref().unwrap_or("none"),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequiredReactorPolicyError {
    pub(crate) snapshot: SchedulingDiagnosticSnapshot,
}

impl fmt::Display for RequiredReactorPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "required native reactor SCHED_RR priority 1 with RESET_ON_FORK was unavailable: {}",
            self.snapshot.detail()
        )
    }
}

impl std::error::Error for RequiredReactorPolicyError {}

trait SchedulingOperations {
    fn inspect(&mut self, tid: i32) -> Result<KernelSchedulingState, String>;
    fn prepare_realtime_safety(&mut self, tid: i32) -> Result<(), String>;
    fn restore_realtime_safety(&mut self, tid: i32);
    fn request_realtime_round_robin(
        &mut self,
        tid: i32,
        priority: i32,
        reset_on_fork: bool,
    ) -> Result<(), String>;
    fn request_rtkit_high_priority_fair(&mut self, tid: i32, nice: i32) -> Result<(), String>;
    fn request_direct_high_priority_fair(&mut self, tid: i32, nice: i32) -> Result<(), String>;
    fn request_normal_fair(&mut self, tid: i32, nice: i32) -> Result<(), String>;
    fn demotion_count(&self, tid: i32) -> u64;
    fn rt_budget_overrun(&self, tid: i32) -> bool;
}

fn configure_role(
    role: SchedulingThreadRole,
    mode: SchedulingQosPolicy,
    tid: i32,
    baseline_nice: i32,
    ops: &mut impl SchedulingOperations,
) -> Result<SchedulingDiagnosticSnapshot, RequiredReactorPolicyError> {
    configure_role_with_rtkit(role, mode, tid, baseline_nice, true, ops)
}

fn configure_role_with_rtkit(
    role: SchedulingThreadRole,
    mode: SchedulingQosPolicy,
    tid: i32,
    baseline_nice: i32,
    allow_reactor_rtkit: bool,
    ops: &mut impl SchedulingOperations,
) -> Result<SchedulingDiagnosticSnapshot, RequiredReactorPolicyError> {
    let requested_class = if mode == SchedulingQosPolicy::Off {
        SchedulingClass::Normal
    } else {
        SchedulingClass::requested_for(role, baseline_nice)
    };
    let mut degradation = Vec::new();
    let mut backend = SchedulingBackend::Inherited;

    let mut effective = match ops.inspect(tid) {
        Ok(state) => state,
        Err(error) => {
            degradation.push(format!("initial scheduler query failed: {error}"));
            KernelSchedulingState {
                policy: PolicyKind::Unknown(-1),
                priority: -1,
                nice: baseline_nice,
                reset_on_fork: false,
            }
        }
    };

    if mode == SchedulingQosPolicy::Off {
        return Ok(snapshot(
            role,
            tid,
            requested_class,
            effective,
            backend,
            ops,
            None,
        ));
    }

    match role {
        SchedulingThreadRole::NativeReactor => {
            let mut inherited_rr_safe = false;
            if EffectiveSchedulingClass::from_kernel(effective)
                .is_expected_reactor_rr(effective.reset_on_fork)
            {
                match ops.prepare_realtime_safety(tid) {
                    Ok(()) => {
                        backend = SchedulingBackend::Inherited;
                        inherited_rr_safe = true;
                    }
                    Err(error) => {
                        degradation
                            .push(format!("inherited SCHED_RR safety setup failed: {error}"));
                        let _ = ops.request_normal_fair(tid, baseline_nice);
                        if let Ok(actual) = ops.inspect(tid) {
                            effective = actual;
                        }
                        backend = SchedulingBackend::None;
                    }
                }
            } else {
                match ops.request_realtime_round_robin(tid, REACTOR_RR_PRIORITY, true) {
                    Ok(()) => match ops.inspect(tid) {
                        Ok(actual)
                            if EffectiveSchedulingClass::from_kernel(actual)
                                .is_expected_reactor_rr(actual.reset_on_fork) =>
                        {
                            effective = actual;
                            backend = SchedulingBackend::Direct;
                        }
                        Ok(actual) => {
                            degradation.push(format!(
                                "SCHED_RR request did not verify (policy={} priority={} reset_on_fork={})",
                                EffectiveSchedulingClass::from_kernel(actual).as_str(),
                                actual.priority,
                                actual.reset_on_fork
                            ));
                            // A successful syscall whose resulting state cannot
                            // be verified must not leave either the RT policy or
                            // its process-wide safety limit active while falling
                            // back to FAIR.
                            let _ = ops.request_normal_fair(tid, baseline_nice);
                            ops.restore_realtime_safety(tid);
                            effective = ops.inspect(tid).unwrap_or(actual);
                        }
                        Err(error) => {
                            degradation.push(format!("SCHED_RR verification failed: {error}"));
                            let _ = ops.request_normal_fair(tid, baseline_nice);
                            ops.restore_realtime_safety(tid);
                            if let Ok(actual) = ops.inspect(tid) {
                                effective = actual;
                            }
                        }
                    },
                    Err(error) => {
                        ops.restore_realtime_safety(tid);
                        degradation.push(format!("direct SCHED_RR failed: {error}"));
                    }
                }
            }

            let has_verified_rr = inherited_rr_safe
                || (backend == SchedulingBackend::Direct
                    && EffectiveSchedulingClass::from_kernel(effective)
                        .is_expected_reactor_rr(effective.reset_on_fork));
            if has_verified_rr {
                return Ok(snapshot(
                    role,
                    tid,
                    requested_class,
                    effective,
                    backend,
                    ops,
                    None,
                ));
            }

            // `require` means verified RR priority 1 with RESET_ON_FORK. Do not
            // make a fair-policy mutation and then fail startup.
            if mode == SchedulingQosPolicy::Require {
                let reason = Some(join_reasons(&degradation, "required SCHED_RR unavailable"));
                return Err(RequiredReactorPolicyError {
                    snapshot: snapshot(
                        role,
                        tid,
                        requested_class,
                        effective,
                        SchedulingBackend::None,
                        ops,
                        reason,
                    ),
                });
            }

            // RTKit realtime is deliberately excluded until descendant
            // RLIMIT_RTTIME handling is safe. Its high-priority FAIR operation
            // is an allowed, setup-only fallback.
            let fair_nice = HIGH_PRIORITY_NICE;
            let mut fallback_state = SchedulingFallbackState {
                effective: &mut effective,
                backend: &mut backend,
                degradation: &mut degradation,
            };
            let fair_policy_acquired = if allow_reactor_rtkit {
                request_and_verify_fair(
                    ops,
                    tid,
                    fair_nice,
                    baseline_nice,
                    true,
                    &mut fallback_state,
                )
            } else {
                request_and_verify_direct_fair(
                    ops,
                    tid,
                    fair_nice,
                    baseline_nice,
                    &mut fallback_state,
                )
            };
            if fair_policy_acquired {
                return Ok(snapshot(
                    role,
                    tid,
                    requested_class,
                    effective,
                    backend,
                    ops,
                    Some(join_reasons(
                        &degradation,
                        "SCHED_RR unavailable; using FAIR",
                    )),
                ));
            }
        }
        SchedulingThreadRole::KmsCommit | SchedulingThreadRole::Render => {
            if EffectiveSchedulingClass::from_kernel(effective)
                .is_fair_priority_above(baseline_nice)
            {
                return Ok(snapshot(
                    role,
                    tid,
                    requested_class,
                    effective,
                    SchedulingBackend::Inherited,
                    ops,
                    None,
                ));
            }
            let mut fallback_state = SchedulingFallbackState {
                effective: &mut effective,
                backend: &mut backend,
                degradation: &mut degradation,
            };
            let _ = request_and_verify_fair(
                ops,
                tid,
                HIGH_PRIORITY_NICE,
                baseline_nice,
                false,
                &mut fallback_state,
            );
        }
        SchedulingThreadRole::Background => {
            // Background roles are explicitly ordinary. A role-specific
            // background nice policy may be added later, but Phase 1 keeps the
            // creator's ordinary nice value and never requests realtime.
            if !matches!(effective.policy, PolicyKind::Other) || effective.nice != baseline_nice {
                if let Err(error) = ops.request_normal_fair(tid, baseline_nice) {
                    degradation.push(format!(
                        "background worker could not be normalized to SCHED_OTHER nice {baseline_nice}: {error}"
                    ));
                }
                if let Ok(actual) = ops.inspect(tid) {
                    effective = actual;
                }
                backend = SchedulingBackend::Direct;
            }
        }
    }

    let is_high_fair =
        EffectiveSchedulingClass::from_kernel(effective).is_fair_priority_above(baseline_nice);
    let degradation_reason = if is_high_fair {
        (!degradation.is_empty()).then(|| join_reasons(&degradation, "using high-priority FAIR"))
    } else {
        if !matches!(effective.policy, PolicyKind::Other) || effective.nice != baseline_nice {
            let _ = ops.request_normal_fair(tid, baseline_nice);
            match ops.inspect(tid) {
                Ok(actual) => effective = actual,
                Err(error) => degradation.push(format!("final scheduler query failed: {error}")),
            }
        }
        backend = SchedulingBackend::None;
        Some(join_reasons(&degradation, "using ordinary SCHED_OTHER"))
    };

    Ok(snapshot(
        role,
        tid,
        requested_class,
        effective,
        backend,
        ops,
        degradation_reason,
    ))
}

fn normalized_spawn_creator_state(
    creator: KernelSchedulingState,
    baseline_nice: i32,
) -> KernelSchedulingState {
    if matches!(creator.policy, PolicyKind::Other) {
        // A high-priority FAIR creator cannot leak realtime scheduling. Keep
        // its nice value intact; the child's trampoline raises its own nice
        // value before invoking generic work, preserving an RTKit grant on the
        // reactor without running helper work at that inherited nice value.
        return creator;
    }
    KernelSchedulingState {
        policy: PolicyKind::Other,
        priority: 0,
        nice: creator.nice.max(baseline_nice),
        reset_on_fork: false,
    }
}

struct SchedulingFallbackState<'a> {
    effective: &'a mut KernelSchedulingState,
    backend: &'a mut SchedulingBackend,
    degradation: &'a mut Vec<String>,
}

fn request_and_verify_fair(
    ops: &mut impl SchedulingOperations,
    tid: i32,
    requested_nice: i32,
    baseline_nice: i32,
    rtkit_first: bool,
    fallback: &mut SchedulingFallbackState<'_>,
) -> bool {
    let attempts = if rtkit_first {
        [SchedulingBackend::RealtimeKit, SchedulingBackend::Direct]
    } else {
        [SchedulingBackend::Direct, SchedulingBackend::RealtimeKit]
    };

    for candidate in attempts {
        let result = match candidate {
            SchedulingBackend::RealtimeKit => {
                ops.request_rtkit_high_priority_fair(tid, requested_nice)
            }
            SchedulingBackend::Direct => ops.request_direct_high_priority_fair(tid, requested_nice),
            _ => unreachable!(),
        };
        match result {
            Ok(()) => match ops.inspect(tid) {
                Ok(actual)
                    if matches!(actual.policy, PolicyKind::Other)
                        && actual.nice < baseline_nice =>
                {
                    *fallback.effective = actual;
                    *fallback.backend = candidate;
                    return true;
                }
                Ok(actual) => {
                    fallback.degradation.push(format!(
                        "{} high-priority request did not verify (policy={} nice={})",
                        candidate.as_str(),
                        EffectiveSchedulingClass::from_kernel(actual).as_str(),
                        actual.nice
                    ));
                    *fallback.effective = actual;
                }
                Err(error) => fallback.degradation.push(format!(
                    "{} high-priority verification failed: {error}",
                    candidate.as_str()
                )),
            },
            Err(error) => fallback.degradation.push(format!(
                "{} high-priority request failed: {error}",
                candidate.as_str()
            )),
        }
    }
    false
}

fn request_and_verify_direct_fair(
    ops: &mut impl SchedulingOperations,
    tid: i32,
    requested_nice: i32,
    baseline_nice: i32,
    fallback: &mut SchedulingFallbackState<'_>,
) -> bool {
    match ops.request_direct_high_priority_fair(tid, requested_nice) {
        Ok(()) => match ops.inspect(tid) {
            Ok(actual)
                if matches!(actual.policy, PolicyKind::Other) && actual.nice < baseline_nice =>
            {
                *fallback.effective = actual;
                *fallback.backend = SchedulingBackend::Direct;
                true
            }
            Ok(actual) => {
                fallback.degradation.push(format!(
                    "direct high-priority request did not verify (policy={} nice={})",
                    EffectiveSchedulingClass::from_kernel(actual).as_str(),
                    actual.nice
                ));
                *fallback.effective = actual;
                false
            }
            Err(error) => {
                fallback
                    .degradation
                    .push(format!("direct high-priority verification failed: {error}"));
                false
            }
        },
        Err(error) => {
            fallback
                .degradation
                .push(format!("direct high-priority request failed: {error}"));
            false
        }
    }
}

fn snapshot(
    role: SchedulingThreadRole,
    tid: i32,
    requested_class: SchedulingClass,
    effective: KernelSchedulingState,
    backend: SchedulingBackend,
    ops: &impl SchedulingOperations,
    degradation_reason: Option<String>,
) -> SchedulingDiagnosticSnapshot {
    let fallback_attempted = degradation_reason
        .as_deref()
        .is_some_and(|reason| !reason.contains("required SCHED_RR unavailable"));
    let requested_fallback_nice = (role == SchedulingThreadRole::NativeReactor
        && matches!(
            requested_class,
            SchedulingClass::RealtimeRoundRobin {
                priority: REACTOR_RR_PRIORITY
            }
        )
        && fallback_attempted
        && !EffectiveSchedulingClass::from_kernel(effective)
            .is_expected_reactor_rr(effective.reset_on_fork))
    .then_some(HIGH_PRIORITY_NICE);
    SchedulingDiagnosticSnapshot {
        role,
        tid,
        requested_class,
        effective_class: EffectiveSchedulingClass::from_kernel(effective),
        backend,
        requested_priority: requested_class.requested_priority(),
        requested_nice: requested_class.requested_nice(),
        requested_fallback_nice,
        effective_priority: effective.priority,
        effective_nice: effective.nice,
        reset_on_fork_effective: effective.reset_on_fork,
        demotion_count: ops.demotion_count(tid),
        rt_budget_overrun: ops.rt_budget_overrun(tid),
        degradation_reason,
    }
}

fn join_reasons(reasons: &[String], terminal: &str) -> String {
    if reasons.is_empty() {
        terminal.to_owned()
    } else {
        format!("{}; {terminal}", reasons.join("; "))
    }
}

const QOS_ENV: &str = "OBLIVION_ONE_SCHEDULING_QOS";
const RT_BUDGET_US: u64 = 100_000;
const BROKER_QUEUE_CAPACITY: usize = 8;
const BROKER_REPLY_TIMEOUT: Duration = Duration::from_millis(750);

static MANAGER: OnceLock<QosManager> = OnceLock::new();
static ACTIVE_REACTOR_TID: AtomicI32 = AtomicI32::new(0);
static RT_BUDGET_OVERRUN: AtomicBool = AtomicBool::new(false);
static RT_DEMOTION_COUNT: AtomicU64 = AtomicU64::new(0);

struct BrokerRequest {
    tid: i32,
    requested_nice: i32,
    response: SyncSender<Result<i32, String>>,
}

struct QosManager {
    policy: SchedulingQosPolicy,
    baseline_nice: i32,
    original_rt_limit: Option<libc::rlimit>,
    broker: Mutex<Option<SyncSender<BrokerRequest>>>,
    snapshots: Mutex<BTreeMap<(SchedulingThreadRole, i32), SchedulingDiagnosticSnapshot>>,
    saved_sigxcpu_action: Mutex<Option<libc::sigaction>>,
    saved_reactor_signal_mask: Mutex<Option<libc::sigset_t>>,
    budget_active: AtomicBool,
}

/// Read policy and process limits before native workers start.
/// `off` performs no scheduling mutation.
pub fn initialize_from_environment() -> Result<(), String> {
    if MANAGER.get().is_some() {
        return Ok(());
    }
    let value = std::env::var(QOS_ENV).ok();
    let policy = SchedulingQosPolicy::parse(value.as_deref())?;
    let baseline_nice = current_nice(unsafe { libc::gettid() }).unwrap_or(0);
    let original_rt_limit = if policy == SchedulingQosPolicy::Off {
        None
    } else {
        match get_rt_limit() {
            Ok(limit) => Some(limit),
            Err(error) => {
                eprintln!("scheduling QoS: realtime budget unavailable: {error}");
                None
            }
        }
    };

    MANAGER
        .set(QosManager {
            policy,
            baseline_nice,
            original_rt_limit,
            broker: Mutex::new(None),
            snapshots: Mutex::new(BTreeMap::new()),
            saved_sigxcpu_action: Mutex::new(None),
            saved_reactor_signal_mask: Mutex::new(None),
            budget_active: AtomicBool::new(false),
        })
        .map_err(|_| "scheduling QoS initialized more than once".to_owned())?;

    if policy != SchedulingQosPolicy::Off
        && matches!(inspect_kernel_state(unsafe { libc::gettid() }), Ok(state) if matches!(state.policy, PolicyKind::Other) && state.nice >= baseline_nice)
        && let Some(manager) = MANAGER.get()
    {
        let broker = start_rtkit_broker();
        if let Ok(mut slot) = manager.broker.lock() {
            *slot = broker;
        }
    }
    Ok(())
}

/// Apply and verify the current native-runtime thread's policy. Call at the
/// cycle-loop boundary after workers and child supervisors have been created.
pub fn promote_current_reactor() -> Result<SchedulingDiagnosticSnapshot, Box<dyn std::error::Error>>
{
    let manager = manager()?;
    let tid = unsafe { libc::gettid() };
    // Take the registry while the current thread is still ordinary so
    // publishing verified RR state cannot block behind a lower-priority worker.
    let mut snapshots = manager
        .snapshots
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut operations = LinuxSchedulingOperations { manager };
    let configured = configure_role(
        SchedulingThreadRole::NativeReactor,
        manager.policy,
        tid,
        manager.baseline_nice,
        &mut operations,
    );
    match configured {
        Ok(snapshot) => {
            if !matches!(
                snapshot.effective_class,
                EffectiveSchedulingClass::RealtimeRoundRobin {
                    priority: REACTOR_RR_PRIORITY
                }
            ) || !snapshot.reset_on_fork_effective
            {
                manager.restore_rt_budget();
            } else {
                ACTIVE_REACTOR_TID.store(tid, Ordering::SeqCst);
            }
            snapshots.insert((snapshot.role, snapshot.tid), snapshot.clone());
            Ok(snapshot)
        }
        Err(error) => {
            if manager.policy != SchedulingQosPolicy::Off {
                let tid = unsafe { libc::gettid() };
                let _ = set_scheduler_other(tid);
                let _ = set_nice(tid, manager.baseline_nice);
            }
            manager.restore_rt_budget();
            snapshots.insert(
                (error.snapshot.role, error.snapshot.tid),
                error.snapshot.clone(),
            );
            Err(Box::new(error))
        }
    }
}

/// Reacquire reactor policy after session recovery without making a D-Bus call
/// from the event-loop thread. If direct protection is unavailable, this
/// boundary degrades to direct FAIR or normal SCHED_OTHER.
pub fn reacquire_current_reactor_after_session_resume()
-> Result<SchedulingDiagnosticSnapshot, Box<dyn std::error::Error>> {
    let manager = manager()?;
    let tid = unsafe { libc::gettid() };
    let mut operations = LinuxSchedulingOperations { manager };
    if RT_BUDGET_OVERRUN.load(Ordering::Acquire) {
        let effective = operations.inspect(tid).unwrap_or(KernelSchedulingState {
            policy: PolicyKind::Unknown(-1),
            priority: -1,
            nice: manager.baseline_nice,
            reset_on_fork: false,
        });
        let snapshot = snapshot(
            SchedulingThreadRole::NativeReactor,
            tid,
            SchedulingClass::requested_for(
                SchedulingThreadRole::NativeReactor,
                manager.baseline_nice,
            ),
            effective,
            SchedulingBackend::None,
            &operations,
            Some("RT budget overrun; realtime reacquisition suppressed".to_owned()),
        );
        manager.record(snapshot.clone());
        return Ok(snapshot);
    }
    let mode = if manager.policy == SchedulingQosPolicy::Require {
        SchedulingQosPolicy::Auto
    } else {
        manager.policy
    };
    // Recovery is still normal-priority here. Acquire the snapshot lock before
    // direct reacquisition so the reactor never waits on this lock under RR.
    let mut snapshots = manager
        .snapshots
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let snapshot = configure_role_with_rtkit(
        SchedulingThreadRole::NativeReactor,
        mode,
        tid,
        manager.baseline_nice,
        false,
        &mut operations,
    )
    .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)?;
    if snapshot
        .effective_class
        .is_expected_reactor_rr(snapshot.reset_on_fork_effective)
    {
        ACTIVE_REACTOR_TID.store(tid, Ordering::SeqCst);
    } else {
        manager.restore_rt_budget();
    }
    snapshots.insert((snapshot.role, snapshot.tid), snapshot.clone());
    Ok(snapshot)
}

/// Demote before controlled shutdown joins or emergency teardown.
pub fn demote_current_reactor() {
    let Some(manager) = MANAGER.get() else {
        return;
    };
    if manager.policy == SchedulingQosPolicy::Off {
        return;
    }
    let tid = unsafe { libc::gettid() };
    let mut operations = LinuxSchedulingOperations { manager };
    let _ = operations.request_normal_fair(tid, manager.baseline_nice);
    ACTIVE_REACTOR_TID.store(0, Ordering::SeqCst);
    manager.restore_rt_budget();
    if let Ok(state) = operations.inspect(tid) {
        let previous = manager.snapshots.lock().ok().and_then(|snapshots| {
            snapshots
                .get(&(SchedulingThreadRole::NativeReactor, tid))
                .cloned()
        });
        if let Some(mut snapshot) = previous {
            snapshot.effective_class = EffectiveSchedulingClass::from_kernel(state);
            snapshot.effective_priority = state.priority;
            snapshot.effective_nice = state.nice;
            snapshot.demotion_count = RT_DEMOTION_COUNT.load(Ordering::Relaxed);
            snapshot.rt_budget_overrun = RT_BUDGET_OVERRUN.load(Ordering::Relaxed);
            manager.record(snapshot);
        }
    }
}

/// Called at a normal cycle boundary to retire a signal-path RT budget event.
pub fn observe_rt_budget_event() -> bool {
    let Some(manager) = MANAGER.get() else {
        return false;
    };
    if !RT_BUDGET_OVERRUN.load(Ordering::Relaxed) {
        return false;
    }
    ACTIVE_REACTOR_TID.store(0, Ordering::SeqCst);
    manager.restore_rt_budget();
    if let Ok(tid) = current_tid()
        && let Ok(state) = inspect_kernel_state(tid)
        && let Ok(mut snapshots) = manager.snapshots.try_lock()
        && let Some(snapshot) = snapshots.get_mut(&(SchedulingThreadRole::NativeReactor, tid))
    {
        snapshot.effective_class = EffectiveSchedulingClass::from_kernel(state);
        snapshot.effective_priority = state.priority;
        snapshot.effective_nice = state.nice;
        snapshot.demotion_count = RT_DEMOTION_COUNT.load(Ordering::Relaxed);
        snapshot.rt_budget_overrun = true;
        snapshot.degradation_reason =
            Some("demoted by SIGXCPU after RLIMIT_RTTIME budget".to_owned());
    }
    true
}

pub struct RoleRegistration {
    role: SchedulingThreadRole,
    tid: i32,
}

impl Drop for RoleRegistration {
    fn drop(&mut self) {
        if let Some(manager) = MANAGER.get()
            && let Ok(mut snapshots) = manager.snapshots.lock()
        {
            snapshots.remove(&(self.role, self.tid));
        }
    }
}

/// Configure a worker from inside that worker before it begins role work.
pub fn register_current_thread_role(role: SchedulingThreadRole) -> RoleRegistration {
    let tid = unsafe { libc::gettid() };
    if let Some(manager) = MANAGER.get() {
        let mut operations = LinuxSchedulingOperations { manager };
        let snapshot = configure_role(
            role,
            manager.policy,
            tid,
            manager.baseline_nice,
            &mut operations,
        )
        .unwrap_or_else(|error| error.snapshot);
        manager.record(snapshot);
    }
    RoleRegistration { role, tid }
}

/// Create a classified helper. A realtime creator is temporarily normalized
/// before pthread creation so the child inherits SCHED_OTHER before Rust
/// starts it. The child trampoline normalizes inherited nice before role work.
pub fn spawn_background<T, F>(name: &'static str, work: F) -> io::Result<JoinHandle<T>>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    spawn_role_thread(name, SchedulingThreadRole::Background, work)
}

pub fn spawn_role_thread<T, F>(
    name: &'static str,
    role: SchedulingThreadRole,
    work: F,
) -> io::Result<JoinHandle<T>>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let spawn = || {
        thread::Builder::new().name(name.to_owned()).spawn(move || {
            if let Some(manager) = MANAGER.get()
                && manager.policy != SchedulingQosPolicy::Off
            {
                let tid = unsafe { libc::gettid() };
                let _ = set_nice(tid, manager.baseline_nice);
            }
            let _role = register_current_thread_role(role);
            work()
        })
    };
    match MANAGER.get() {
        Some(manager) if manager.policy != SchedulingQosPolicy::Off => {
            with_normalized_thread_creator(manager, spawn)
        }
        _ => spawn(),
    }
}

fn with_normalized_thread_creator<T>(
    manager: &QosManager,
    spawn: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    let tid = unsafe { libc::gettid() };
    let prior = inspect_kernel_state(tid).map_err(io::Error::other)?;
    let is_reactor = ACTIVE_REACTOR_TID.load(Ordering::Acquire) == tid;
    let can_restore_rt = matches!(prior.policy, PolicyKind::RoundRobin) && is_reactor;
    if matches!(
        prior.policy,
        PolicyKind::Fifo | PolicyKind::Deadline | PolicyKind::Unknown(_)
    ) || (matches!(prior.policy, PolicyKind::RoundRobin) && !can_restore_rt)
    {
        return Err(io::Error::other(
            "refusing worker creation from an unclassified realtime/deadline creator",
        ));
    }

    let desired = normalized_spawn_creator_state(prior, manager.baseline_nice);
    let scheduler_changed = !matches!(prior.policy, PolicyKind::Other);
    let nice_changed = prior.nice != desired.nice;
    if scheduler_changed {
        set_scheduler_other(tid).map_err(io::Error::other)?;
    }
    if nice_changed && let Err(error) = set_nice(tid, desired.nice) {
        if scheduler_changed {
            let _ = restore_thread_scheduler(tid, prior);
        }
        return Err(io::Error::other(error));
    }

    let spawned = spawn();
    if can_restore_rt && ACTIVE_REACTOR_TID.load(Ordering::Acquire) != tid {
        manager.restore_rt_budget();
        record_reactor_degradation(
            manager,
            tid,
            "SIGXCPU demoted reactor during normalized helper creation",
        );
        return spawned;
    }

    let restore_result = (if scheduler_changed {
        restore_thread_scheduler(tid, prior)
    } else {
        Ok(())
    })
    .and_then(|()| {
        if nice_changed {
            set_nice(tid, prior.nice).map_err(io::Error::other)
        } else {
            Ok(())
        }
    });
    if let Err(error) = restore_result {
        let _ = set_scheduler_other(tid);
        let _ = set_nice(tid, manager.baseline_nice);
        if can_restore_rt {
            manager.restore_rt_budget();
        }
        record_reactor_degradation(
            manager,
            tid,
            &format!("creator QoS restoration failed after worker start: {error}"),
        );
        return spawned;
    }
    // The SIGXCPU handler clears ACTIVE_REACTOR_TID before attempting its
    // syscall. If it raced with this restoration, undo any re-promotion. If it
    // races after this check, its own syscall demotes the restored reactor.
    if can_restore_rt && ACTIVE_REACTOR_TID.load(Ordering::Acquire) != tid {
        let _ = set_scheduler_other(tid);
        let _ = set_nice(tid, manager.baseline_nice);
        manager.restore_rt_budget();
        record_reactor_degradation(
            manager,
            tid,
            "SIGXCPU demoted reactor during normalized helper creation",
        );
    }
    spawned
}

fn restore_thread_scheduler(tid: i32, state: KernelSchedulingState) -> Result<(), io::Error> {
    let (base_policy, allowed) = match state.policy {
        PolicyKind::RoundRobin => (libc::SCHED_RR, true),
        PolicyKind::Fifo => (libc::SCHED_FIFO, true),
        PolicyKind::Other => (libc::SCHED_OTHER, true),
        PolicyKind::Deadline | PolicyKind::Unknown(_) => (libc::SCHED_OTHER, false),
    };
    if !allowed {
        return Err(io::Error::other(
            "cannot restore unsupported thread scheduling class",
        ));
    }
    let policy = base_policy
        | if state.reset_on_fork {
            libc::SCHED_RESET_ON_FORK
        } else {
            0
        };
    let parameter = libc::sched_param {
        sched_priority: state.priority,
    };
    if unsafe { libc::sched_setscheduler(tid, policy, &parameter) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn record_reactor_degradation(manager: &QosManager, tid: i32, reason: &str) {
    let Ok(mut snapshots) = manager.snapshots.try_lock() else {
        return;
    };
    let Some(snapshot) = snapshots.get_mut(&(SchedulingThreadRole::NativeReactor, tid)) else {
        return;
    };
    if let Ok(state) = inspect_kernel_state(tid) {
        snapshot.effective_class = EffectiveSchedulingClass::from_kernel(state);
        snapshot.effective_priority = state.priority;
        snapshot.effective_nice = state.nice;
        snapshot.reset_on_fork_effective = state.reset_on_fork;
    }
    snapshot.backend = SchedulingBackend::None;
    snapshot.demotion_count = RT_DEMOTION_COUNT.load(Ordering::Relaxed);
    snapshot.rt_budget_overrun = RT_BUDGET_OVERRUN.load(Ordering::Relaxed);
    snapshot.degradation_reason = Some(reason.to_owned());
}

/// Add child restoration to every managed process launch. This pre-exec hook
/// uses only setrlimit/setpriority and is safe in the post-fork child.
pub(crate) fn prepare_child_scheduling(command: &mut std::process::Command) {
    let Some(manager) = MANAGER.get() else {
        return;
    };
    if manager.policy == SchedulingQosPolicy::Off {
        return;
    }
    let limit = manager.original_rt_limit;
    let nice = manager.baseline_nice;
    use std::os::unix::process::CommandExt;
    // SAFETY: Both operations are async-signal-safe libc syscalls and the
    // closure runs in the child between fork and exec.
    unsafe {
        command.pre_exec(move || {
            if let Some(limit) = limit
                && libc::setrlimit(libc::RLIMIT_RTTIME, &limit) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if libc::setpriority(libc::PRIO_PROCESS, 0, nice) != 0 {
                return Err(io::Error::last_os_error());
            }
            let ambient_clear = libc::prctl(
                libc::PR_CAP_AMBIENT,
                libc::PR_CAP_AMBIENT_CLEAR_ALL,
                0,
                0,
                0,
            );
            if ambient_clear != 0 {
                let error = io::Error::last_os_error();
                // Kernels without ambient-capability support cannot leak an
                // ambient set because none can be represented there.
                if error.raw_os_error() != Some(libc::EINVAL) {
                    return Err(error);
                }
            }
            Ok(())
        });
    }
}

/// Current live scheduling snapshots, refreshed only on doctor/status reads.
pub fn diagnostic_snapshots() -> Vec<SchedulingDiagnosticSnapshot> {
    let Some(manager) = MANAGER.get() else {
        return Vec::new();
    };
    let mut snapshots = match try_clone_snapshot_registry(&manager.snapshots) {
        Some(snapshots) => snapshots,
        None => {
            let tid = unsafe { libc::gettid() };
            let mut operations = LinuxSchedulingOperations { manager };
            let effective = operations.inspect(tid).unwrap_or(KernelSchedulingState {
                policy: PolicyKind::Unknown(-1),
                priority: -1,
                nice: manager.baseline_nice,
                reset_on_fork: false,
            });
            let requested = if manager.policy == SchedulingQosPolicy::Off {
                SchedulingClass::Normal
            } else {
                SchedulingClass::requested_for(
                    SchedulingThreadRole::NativeReactor,
                    manager.baseline_nice,
                )
            };
            let mut reactor_snapshot = snapshot(
                SchedulingThreadRole::NativeReactor,
                tid,
                requested,
                effective,
                SchedulingBackend::None,
                &operations,
                Some("diagnostic registry busy; effective state sampled directly".to_owned()),
            );
            reactor_snapshot.requested_fallback_nice = None;
            return vec![reactor_snapshot];
        }
    };
    for snapshot in snapshots.values_mut() {
        if let Ok(state) = inspect_kernel_state(snapshot.tid) {
            snapshot.effective_class = EffectiveSchedulingClass::from_kernel(state);
            snapshot.effective_priority = state.priority;
            snapshot.effective_nice = state.nice;
            snapshot.reset_on_fork_effective = state.reset_on_fork;
        } else {
            snapshot.degradation_reason = Some("thread ended before diagnostic query".to_owned());
        }
        if snapshot.role == SchedulingThreadRole::NativeReactor {
            snapshot.demotion_count = RT_DEMOTION_COUNT.load(Ordering::Relaxed);
            snapshot.rt_budget_overrun = RT_BUDGET_OVERRUN.load(Ordering::Relaxed);
            if snapshot.rt_budget_overrun {
                snapshot.backend = SchedulingBackend::None;
                snapshot.degradation_reason =
                    Some("demoted by SIGXCPU after RLIMIT_RTTIME budget".to_owned());
            }
        }
    }
    snapshots.into_values().collect()
}

fn try_clone_snapshot_registry(
    snapshots: &Mutex<BTreeMap<(SchedulingThreadRole, i32), SchedulingDiagnosticSnapshot>>,
) -> Option<BTreeMap<(SchedulingThreadRole, i32), SchedulingDiagnosticSnapshot>> {
    match snapshots.try_lock() {
        Ok(snapshots) => Some(snapshots.clone()),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner().clone()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

fn manager() -> Result<&'static QosManager, io::Error> {
    MANAGER
        .get()
        .ok_or_else(|| io::Error::other("scheduling QoS was not initialized"))
}

impl QosManager {
    fn record(&self, snapshot: SchedulingDiagnosticSnapshot) {
        if let Ok(mut snapshots) = self.snapshots.lock() {
            snapshots.insert((snapshot.role, snapshot.tid), snapshot);
        }
    }

    fn prepare_rt_budget(&self, tid: i32) -> Result<(), String> {
        if self.budget_active.load(Ordering::Acquire) {
            ACTIVE_REACTOR_TID.store(tid, Ordering::SeqCst);
            return Ok(());
        }
        let original_rt_limit = self
            .original_rt_limit
            .ok_or_else(|| "RLIMIT_RTTIME could not be queried; realtime disabled".to_owned())?;
        let guarded = guarded_rt_limit(original_rt_limit, RT_BUDGET_US)?;
        let mut signal_set = unsafe { std::mem::zeroed::<libc::sigset_t>() };
        if unsafe { libc::sigemptyset(&mut signal_set) } != 0
            || unsafe { libc::sigaddset(&mut signal_set, libc::SIGXCPU) } != 0
        {
            return Err(format!(
                "cannot prepare SIGXCPU signal mask: {}",
                io::Error::last_os_error()
            ));
        }
        let mut previous_mask = unsafe { std::mem::zeroed::<libc::sigset_t>() };
        let block_result =
            unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &signal_set, &mut previous_mask) };
        if block_result != 0 {
            return Err(format!(
                "cannot block SIGXCPU during fail-safe setup: {}",
                io::Error::from_raw_os_error(block_result)
            ));
        }

        let mut previous = unsafe { std::mem::zeroed::<libc::sigaction>() };
        let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
        action.sa_sigaction = sigxcpu_demote as *const () as usize;
        action.sa_flags = libc::SA_RESTART;
        unsafe { libc::sigemptyset(&mut action.sa_mask) };
        // SAFETY: The handler only performs lock-free atomic operations and a
        // direct scheduler syscall. SIGXCPU is not owned by the runtime signalfd.
        if unsafe { libc::sigaction(libc::SIGXCPU, &action, &mut previous) } != 0 {
            unsafe {
                libc::pthread_sigmask(libc::SIG_SETMASK, &previous_mask, std::ptr::null_mut());
            }
            return Err(format!(
                "cannot install SIGXCPU fail-safe: {}",
                io::Error::last_os_error()
            ));
        }
        if unsafe { libc::setrlimit(libc::RLIMIT_RTTIME, &guarded) } != 0 {
            let error = io::Error::last_os_error();
            unsafe {
                libc::sigaction(libc::SIGXCPU, &previous, std::ptr::null_mut());
                libc::pthread_sigmask(libc::SIG_SETMASK, &previous_mask, std::ptr::null_mut());
            }
            return Err(format!(
                "cannot set finite RLIMIT_RTTIME soft budget: {error}"
            ));
        }
        match self.saved_reactor_signal_mask.lock() {
            Ok(mut saved_mask) => *saved_mask = Some(previous_mask),
            Err(_) => {
                unsafe {
                    libc::setrlimit(libc::RLIMIT_RTTIME, &original_rt_limit);
                    libc::sigaction(libc::SIGXCPU, &previous, std::ptr::null_mut());
                    libc::pthread_sigmask(libc::SIG_SETMASK, &previous_mask, std::ptr::null_mut());
                }
                return Err("SIGXCPU signal-mask state is poisoned".to_owned());
            }
        }
        match self.saved_sigxcpu_action.lock() {
            Ok(mut saved) => *saved = Some(previous),
            Err(_) => {
                if let Ok(mut saved_mask) = self.saved_reactor_signal_mask.lock() {
                    saved_mask.take();
                }
                unsafe {
                    libc::setrlimit(libc::RLIMIT_RTTIME, &original_rt_limit);
                    libc::sigaction(libc::SIGXCPU, &previous, std::ptr::null_mut());
                    libc::pthread_sigmask(libc::SIG_SETMASK, &previous_mask, std::ptr::null_mut());
                }
                return Err("SIGXCPU handler state is poisoned".to_owned());
            }
        }
        RT_BUDGET_OVERRUN.store(false, Ordering::Relaxed);
        ACTIVE_REACTOR_TID.store(tid, Ordering::SeqCst);
        self.budget_active.store(true, Ordering::Release);
        let unblock_result =
            unsafe { libc::pthread_sigmask(libc::SIG_UNBLOCK, &signal_set, std::ptr::null_mut()) };
        if unblock_result != 0 {
            self.restore_rt_budget();
            return Err(format!(
                "cannot unblock SIGXCPU for the realtime reactor: {}",
                io::Error::from_raw_os_error(unblock_result)
            ));
        }
        Ok(())
    }

    fn restore_rt_budget(&self) {
        if !self.budget_active.swap(false, Ordering::AcqRel) {
            return;
        }
        ACTIVE_REACTOR_TID.store(0, Ordering::SeqCst);
        let mut signal_set = unsafe { std::mem::zeroed::<libc::sigset_t>() };
        if unsafe { libc::sigemptyset(&mut signal_set) } == 0
            && unsafe { libc::sigaddset(&mut signal_set, libc::SIGXCPU) } == 0
        {
            let block_result = unsafe {
                libc::pthread_sigmask(libc::SIG_BLOCK, &signal_set, std::ptr::null_mut())
            };
            if block_result != 0 {
                eprintln!(
                    "scheduling QoS: failed to block SIGXCPU during fail-safe cleanup: {}",
                    io::Error::from_raw_os_error(block_result)
                );
            }
        }
        if let Some(original_rt_limit) = self.original_rt_limit
            && unsafe { libc::setrlimit(libc::RLIMIT_RTTIME, &original_rt_limit) } != 0
        {
            eprintln!(
                "scheduling QoS: failed to restore RLIMIT_RTTIME: {}",
                io::Error::last_os_error()
            );
        }
        if let Ok(mut saved) = self.saved_sigxcpu_action.lock()
            && let Some(previous) = saved.take()
            && unsafe { libc::sigaction(libc::SIGXCPU, &previous, std::ptr::null_mut()) } != 0
        {
            eprintln!(
                "scheduling QoS: failed to restore SIGXCPU action: {}",
                io::Error::last_os_error()
            );
        }
        if let Ok(mut saved_mask) = self.saved_reactor_signal_mask.lock()
            && let Some(previous_mask) = saved_mask.take()
        {
            // Restore the prior action before unblocking/restoring the prior
            // mask so a pending SIGXCPU follows the process's original policy.
            let mask_result = unsafe {
                libc::pthread_sigmask(libc::SIG_SETMASK, &previous_mask, std::ptr::null_mut())
            };
            if mask_result != 0 {
                eprintln!(
                    "scheduling QoS: failed to restore reactor signal mask: {}",
                    io::Error::from_raw_os_error(mask_result)
                );
            }
        }
    }
}

struct LinuxSchedulingOperations<'a> {
    manager: &'a QosManager,
}

impl SchedulingOperations for LinuxSchedulingOperations<'_> {
    fn inspect(&mut self, tid: i32) -> Result<KernelSchedulingState, String> {
        inspect_kernel_state(tid)
    }

    fn prepare_realtime_safety(&mut self, tid: i32) -> Result<(), String> {
        self.manager.prepare_rt_budget(tid)
    }

    fn restore_realtime_safety(&mut self, _tid: i32) {
        self.manager.restore_rt_budget();
    }

    fn request_realtime_round_robin(
        &mut self,
        tid: i32,
        priority: i32,
        reset_on_fork: bool,
    ) -> Result<(), String> {
        let minimum = unsafe { libc::sched_get_priority_min(libc::SCHED_RR) };
        if minimum < 0 {
            return Err(format!(
                "sched_get_priority_min(SCHED_RR) failed: {}",
                io::Error::last_os_error()
            ));
        }
        if priority != minimum {
            return Err(format!(
                "requested SCHED_RR priority {priority} is not kernel minimum {minimum}"
            ));
        }
        let parameter = libc::sched_param {
            sched_priority: priority,
        };
        let policy = libc::SCHED_RR
            | if reset_on_fork {
                libc::SCHED_RESET_ON_FORK
            } else {
                0
            };
        self.manager.prepare_rt_budget(tid)?;
        if unsafe { libc::sched_setscheduler(tid, policy, &parameter) } != 0 {
            let error = io::Error::last_os_error().to_string();
            return Err(error);
        }
        Ok(())
    }

    fn request_rtkit_high_priority_fair(&mut self, tid: i32, nice: i32) -> Result<(), String> {
        set_scheduler_other(tid)?;
        self.manager.restore_rt_budget();
        set_nice(tid, self.manager.baseline_nice)?;
        self.manager.request_rtkit_high_priority_fair(tid, nice)
    }

    fn request_direct_high_priority_fair(&mut self, tid: i32, nice: i32) -> Result<(), String> {
        set_scheduler_other(tid)?;
        self.manager.restore_rt_budget();
        set_nice(tid, nice)
    }

    fn request_normal_fair(&mut self, tid: i32, nice: i32) -> Result<(), String> {
        set_scheduler_other(tid)?;
        self.manager.restore_rt_budget();
        set_nice(tid, nice)
    }

    fn demotion_count(&self, _tid: i32) -> u64 {
        RT_DEMOTION_COUNT.load(Ordering::Relaxed)
    }

    fn rt_budget_overrun(&self, _tid: i32) -> bool {
        RT_BUDGET_OVERRUN.load(Ordering::Relaxed)
    }
}

impl QosManager {
    fn request_rtkit_high_priority_fair(&self, tid: i32, nice: i32) -> Result<(), String> {
        let broker_state = self
            .broker
            .lock()
            .map_err(|_| "RTKit broker state is poisoned".to_owned())?;
        let broker = broker_state
            .as_ref()
            .ok_or_else(|| "RTKit broker unavailable".to_owned())?;
        let (response, result) = mpsc::sync_channel(1);
        broker
            .try_send(BrokerRequest {
                tid,
                requested_nice: nice,
                response,
            })
            .map_err(|error| format!("RTKit broker queue unavailable: {error}"))?;
        result
            .recv_timeout(BROKER_REPLY_TIMEOUT)
            .map_err(|error| format!("RTKit broker reply unavailable: {error}"))??;
        Ok(())
    }
}

fn start_rtkit_broker() -> Option<SyncSender<BrokerRequest>> {
    let (sender, receiver) = mpsc::sync_channel::<BrokerRequest>(BROKER_QUEUE_CAPACITY);
    spawn_role_thread(
        "typhon-qos-broker",
        SchedulingThreadRole::Background,
        move || {
            while let Ok(request) = receiver.recv() {
                let result = make_thread_high_priority(request.tid, request.requested_nice);
                let _ = request.response.try_send(result);
            }
        },
    )
    .ok()?;
    Some(sender)
}

fn make_thread_high_priority(tid: i32, requested_nice: i32) -> Result<i32, String> {
    use zbus::Proxy;
    futures_lite::future::block_on(async move {
        let connection = zbus::connection::Builder::system()
            .map_err(|error| error.to_string())?
            .method_timeout(Duration::from_millis(600))
            .build()
            .await
            .map_err(|error| error.to_string())?;
        let proxy = Proxy::new(
            &connection,
            "org.freedesktop.RealtimeKit1",
            "/org/freedesktop/RealtimeKit1",
            "org.freedesktop.RealtimeKit1",
        )
        .await
        .map_err(|error| error.to_string())?;
        let min_nice: i32 = proxy
            .get_property("MinNiceLevel")
            .await
            .map_err(|error| error.to_string())?;
        let nice = requested_nice.max(min_nice);
        proxy
            .call_method("MakeThreadHighPriority", &(tid as u64, nice))
            .await
            .map_err(|error| error.to_string())?;
        Ok(nice)
    })
}

extern "C" fn sigxcpu_demote(_signal: libc::c_int) {
    let tid = ACTIVE_REACTOR_TID.swap(0, Ordering::Relaxed);
    if tid <= 0 {
        return;
    }
    let parameter = libc::sched_param { sched_priority: 0 };
    // SAFETY: Linux syscall used directly from the signal path; no allocation,
    // Rust mutex, logging, filesystem, or D-Bus activity occurs here.
    let result = unsafe {
        libc::syscall(
            libc::SYS_sched_setscheduler,
            tid,
            libc::SCHED_OTHER,
            &parameter,
        )
    };
    if result < 0 {
        unsafe { libc::_exit(128 + libc::SIGXCPU) };
    }
    RT_BUDGET_OVERRUN.store(true, Ordering::Relaxed);
    RT_DEMOTION_COUNT.fetch_add(1, Ordering::Relaxed);
}

fn inspect_kernel_state(tid: i32) -> Result<KernelSchedulingState, String> {
    let raw_policy = unsafe { libc::sched_getscheduler(tid) };
    if raw_policy < 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    let mut parameter = libc::sched_param { sched_priority: 0 };
    if unsafe { libc::sched_getparam(tid, &mut parameter) } != 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    let nice = current_nice(tid).map_err(|error| error.to_string())?;
    let reset_on_fork = raw_policy & libc::SCHED_RESET_ON_FORK != 0;
    let policy = match raw_policy & !libc::SCHED_RESET_ON_FORK {
        libc::SCHED_RR => PolicyKind::RoundRobin,
        libc::SCHED_FIFO => PolicyKind::Fifo,
        libc::SCHED_DEADLINE => PolicyKind::Deadline,
        libc::SCHED_OTHER => PolicyKind::Other,
        other => PolicyKind::Unknown(other),
    };
    Ok(KernelSchedulingState {
        policy,
        priority: parameter.sched_priority,
        nice,
        reset_on_fork,
    })
}

fn current_nice(tid: i32) -> Result<i32, io::Error> {
    #[cfg(target_os = "linux")]
    unsafe {
        *libc::__errno_location() = 0;
    }
    let nice = unsafe { libc::getpriority(libc::PRIO_PROCESS, tid as libc::id_t) };
    if nice == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error().unwrap_or(0) != 0 {
            return Err(error);
        }
    }
    Ok(nice)
}

fn set_nice(tid: i32, nice: i32) -> Result<(), String> {
    if unsafe { libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, nice) } != 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    Ok(())
}

fn set_scheduler_other(tid: i32) -> Result<(), String> {
    let parameter = libc::sched_param { sched_priority: 0 };
    if unsafe { libc::sched_setscheduler(tid, libc::SCHED_OTHER, &parameter) } != 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    Ok(())
}

fn current_tid() -> Result<i32, String> {
    let tid = unsafe { libc::gettid() };
    if tid <= 0 {
        Err(io::Error::last_os_error().to_string())
    } else {
        Ok(tid)
    }
}

fn get_rt_limit() -> Result<libc::rlimit, String> {
    let mut limit = unsafe { std::mem::zeroed::<libc::rlimit>() };
    if unsafe { libc::getrlimit(libc::RLIMIT_RTTIME, &mut limit) } != 0 {
        return Err(format!(
            "getrlimit(RLIMIT_RTTIME) failed: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(limit)
}

fn guarded_rt_limit(original: libc::rlimit, budget_us: u64) -> Result<libc::rlimit, String> {
    let requested = libc::rlim_t::try_from(budget_us).unwrap_or(libc::RLIM_INFINITY);
    let mut guarded = original;
    guarded.rlim_cur = original.rlim_cur.min(original.rlim_max).min(requested);
    if guarded.rlim_cur == 0 {
        return Err("RLIMIT_RTTIME soft budget is zero; realtime disabled".to_owned());
    }
    Ok(guarded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct FakeOps {
        state: KernelSchedulingState,
        rr_result: Result<(), &'static str>,
        rtkit_result: Result<(), &'static str>,
        direct_fair_result: Result<(), &'static str>,
        rr_applies: bool,
        rr_mutations: usize,
        fair_mutations: Vec<SchedulingBackend>,
        normal_mutations: usize,
        safety_restores: usize,
        demotions: u64,
        overrun: bool,
    }

    impl FakeOps {
        fn normal(nice: i32) -> Self {
            Self {
                state: KernelSchedulingState {
                    policy: PolicyKind::Other,
                    priority: 0,
                    nice,
                    reset_on_fork: false,
                },
                rr_result: Ok(()),
                rtkit_result: Err("RTKit unavailable"),
                direct_fair_result: Err("setpriority denied"),
                rr_applies: true,
                rr_mutations: 0,
                fair_mutations: Vec::new(),
                normal_mutations: 0,
                safety_restores: 0,
                demotions: 0,
                overrun: false,
            }
        }

        fn with_fair_success(mut self, backend: SchedulingBackend, nice: i32) -> Self {
            match backend {
                SchedulingBackend::RealtimeKit => self.rtkit_result = Ok(()),
                SchedulingBackend::Direct => self.direct_fair_result = Ok(()),
                _ => unreachable!(),
            }
            self.state.nice = nice;
            self
        }
    }

    impl SchedulingOperations for FakeOps {
        fn inspect(&mut self, _tid: i32) -> Result<KernelSchedulingState, String> {
            Ok(self.state)
        }

        fn prepare_realtime_safety(&mut self, _tid: i32) -> Result<(), String> {
            Ok(())
        }

        fn restore_realtime_safety(&mut self, _tid: i32) {
            self.safety_restores += 1;
        }

        fn request_realtime_round_robin(
            &mut self,
            _tid: i32,
            priority: i32,
            reset_on_fork: bool,
        ) -> Result<(), String> {
            self.rr_mutations += 1;
            self.rr_result.map_err(str::to_owned)?;
            if self.rr_applies {
                self.state = KernelSchedulingState {
                    policy: PolicyKind::RoundRobin,
                    priority,
                    nice: self.state.nice,
                    reset_on_fork,
                };
            }
            Ok(())
        }

        fn request_rtkit_high_priority_fair(
            &mut self,
            _tid: i32,
            _nice: i32,
        ) -> Result<(), String> {
            self.fair_mutations.push(SchedulingBackend::RealtimeKit);
            self.rtkit_result.map_err(str::to_owned)
        }

        fn request_direct_high_priority_fair(
            &mut self,
            _tid: i32,
            _nice: i32,
        ) -> Result<(), String> {
            self.fair_mutations.push(SchedulingBackend::Direct);
            self.direct_fair_result.map_err(str::to_owned)
        }

        fn request_normal_fair(&mut self, _tid: i32, nice: i32) -> Result<(), String> {
            self.normal_mutations += 1;
            self.state = KernelSchedulingState {
                policy: PolicyKind::Other,
                priority: 0,
                nice,
                reset_on_fork: false,
            };
            Ok(())
        }

        fn demotion_count(&self, _tid: i32) -> u64 {
            self.demotions
        }

        fn rt_budget_overrun(&self, _tid: i32) -> bool {
            self.overrun
        }
    }

    #[test]
    fn realtime_budget_only_lowers_soft_limit_and_respects_existing_limits() {
        let original = libc::rlimit {
            rlim_cur: 500_000,
            rlim_max: 1_000_000,
        };
        let guarded = guarded_rt_limit(original, RT_BUDGET_US).unwrap();
        assert_eq!(guarded.rlim_cur, RT_BUDGET_US as libc::rlim_t);
        assert_eq!(guarded.rlim_max, original.rlim_max);

        let smaller_soft = libc::rlimit {
            rlim_cur: 50_000,
            rlim_max: 1_000_000,
        };
        assert_eq!(
            guarded_rt_limit(smaller_soft, RT_BUDGET_US)
                .unwrap()
                .rlim_cur,
            smaller_soft.rlim_cur
        );

        let finite_hard = libc::rlimit {
            rlim_cur: libc::RLIM_INFINITY,
            rlim_max: 75_000,
        };
        assert_eq!(
            guarded_rt_limit(finite_hard, RT_BUDGET_US)
                .unwrap()
                .rlim_cur,
            finite_hard.rlim_max
        );

        let disabled = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 1_000_000,
        };
        assert!(guarded_rt_limit(disabled, RT_BUDGET_US).is_err());
    }

    #[test]
    fn off_requests_no_scheduling_mutation() {
        let mut ops = FakeOps::normal(0);
        let snapshot = configure_role(
            SchedulingThreadRole::NativeReactor,
            SchedulingQosPolicy::Off,
            41,
            0,
            &mut ops,
        )
        .expect("off policy does not fail startup");

        assert_eq!(ops.rr_mutations, 0);
        assert!(ops.fair_mutations.is_empty());
        assert_eq!(ops.normal_mutations, 0);
        assert_eq!(snapshot.backend, SchedulingBackend::Inherited);
        assert_eq!(
            snapshot.effective_class,
            EffectiveSchedulingClass::Fair { nice: 0 }
        );
    }

    #[test]
    fn auto_direct_rr_success_records_kernel_state_and_priority_one() {
        let mut ops = FakeOps::normal(0);
        let snapshot = configure_role(
            SchedulingThreadRole::NativeReactor,
            SchedulingQosPolicy::Auto,
            42,
            0,
            &mut ops,
        )
        .expect("auto mode is best effort");

        assert_eq!(snapshot.backend, SchedulingBackend::Direct);
        assert_eq!(
            snapshot.requested_class,
            SchedulingClass::RealtimeRoundRobin { priority: 1 }
        );
        assert_eq!(
            snapshot.effective_class,
            EffectiveSchedulingClass::RealtimeRoundRobin { priority: 1 }
        );
        assert!(snapshot.reset_on_fork_effective);
        assert_eq!(ops.rr_mutations, 1);
        let detail = snapshot.detail();
        assert!(detail.contains("requested_class=sched_rr"));
        assert!(detail.contains("effective_class=sched_rr"));
        assert!(detail.contains("backend=direct"));
        assert!(detail.contains("reset_on_fork_effective=true"));
    }

    #[test]
    fn direct_rr_permission_failure_falls_back_without_startup_failure() {
        let mut ops = FakeOps::normal(0);
        ops.rr_result = Err("EPERM");
        ops.direct_fair_result = Ok(());
        ops.state.nice = -8;
        let snapshot = configure_role(
            SchedulingThreadRole::NativeReactor,
            SchedulingQosPolicy::Auto,
            43,
            0,
            &mut ops,
        )
        .expect("auto mode must fail open");

        assert_eq!(snapshot.backend, SchedulingBackend::Direct);
        assert_eq!(snapshot.requested_fallback_nice, Some(-10));
        assert_eq!(
            snapshot.effective_class,
            EffectiveSchedulingClass::Fair { nice: -8 }
        );
        assert!(snapshot.degradation_reason.is_some());
        assert_eq!(ops.safety_restores, 1);
    }

    #[test]
    fn rtkit_high_priority_fair_success_records_actual_nice() {
        let mut ops = FakeOps::normal(0).with_fair_success(SchedulingBackend::RealtimeKit, -7);
        ops.rr_result = Err("EPERM");
        let snapshot = configure_role(
            SchedulingThreadRole::NativeReactor,
            SchedulingQosPolicy::Auto,
            44,
            0,
            &mut ops,
        )
        .expect("fair fallback is best effort");

        assert_eq!(snapshot.backend, SchedulingBackend::RealtimeKit);
        assert_eq!(snapshot.effective_nice, -7);
    }

    #[test]
    fn session_resume_reacquisition_uses_no_rtkit_fallback() {
        let mut ops = FakeOps::normal(0);
        ops.rr_result = Err("EPERM");
        ops.rtkit_result = Ok(());
        let snapshot = configure_role_with_rtkit(
            SchedulingThreadRole::NativeReactor,
            SchedulingQosPolicy::Auto,
            53,
            0,
            false,
            &mut ops,
        )
        .expect("session recovery must fail open when direct QoS is unavailable");

        assert_eq!(snapshot.backend, SchedulingBackend::None);
        assert_eq!(ops.fair_mutations, vec![SchedulingBackend::Direct]);
        assert_eq!(
            snapshot.effective_class,
            EffectiveSchedulingClass::Fair { nice: 0 }
        );
    }

    #[test]
    fn total_qos_failure_keeps_functional_normal_scheduling() {
        let mut ops = FakeOps::normal(0);
        ops.rr_result = Err("EPERM");
        let snapshot = configure_role(
            SchedulingThreadRole::NativeReactor,
            SchedulingQosPolicy::Auto,
            45,
            0,
            &mut ops,
        )
        .expect("all QoS failures are non-fatal in auto mode");

        assert_eq!(snapshot.backend, SchedulingBackend::None);
        assert_eq!(
            snapshot.effective_class,
            EffectiveSchedulingClass::Fair { nice: 0 }
        );
        assert!(snapshot.degradation_reason.is_some());
    }

    #[test]
    fn require_rejects_missing_required_reactor_rr_protection() {
        let mut ops = FakeOps::normal(0);
        ops.rr_result = Err("EPERM");
        let error = configure_role(
            SchedulingThreadRole::NativeReactor,
            SchedulingQosPolicy::Require,
            46,
            0,
            &mut ops,
        )
        .expect_err("require mode requires verified SCHED_RR");

        assert_eq!(
            error.snapshot.effective_class,
            EffectiveSchedulingClass::Fair { nice: 0 }
        );
        assert_eq!(error.snapshot.requested_fallback_nice, None);
    }

    #[test]
    fn requested_and_effective_policy_are_independently_verified() {
        let mut ops = FakeOps::normal(0);
        ops.rr_result = Ok(());
        ops.rr_applies = false;
        ops.rtkit_result = Err("RTKit unavailable");
        ops.direct_fair_result = Err("EPERM");
        ops.state.policy = PolicyKind::Other;
        ops.state.nice = 0;
        let snapshot = configure_role(
            SchedulingThreadRole::NativeReactor,
            SchedulingQosPolicy::Auto,
            47,
            0,
            &mut ops,
        )
        .expect("auto mode degrades after an unverifiable request");

        assert_eq!(
            snapshot.requested_class,
            SchedulingClass::RealtimeRoundRobin { priority: 1 }
        );
        assert_eq!(
            snapshot.effective_class,
            EffectiveSchedulingClass::Fair { nice: 0 }
        );
        assert_eq!(snapshot.backend, SchedulingBackend::None);
        assert!(snapshot.degradation_reason.is_some());
    }

    #[test]
    fn kms_and_background_roles_never_request_realtime() {
        let mut kms = FakeOps::normal(0);
        let _ = configure_role(
            SchedulingThreadRole::KmsCommit,
            SchedulingQosPolicy::Auto,
            48,
            0,
            &mut kms,
        );
        let mut background = FakeOps::normal(0);
        let _ = configure_role(
            SchedulingThreadRole::Background,
            SchedulingQosPolicy::Auto,
            49,
            0,
            &mut background,
        );

        assert_eq!(kms.rr_mutations, 0);
        assert_eq!(background.rr_mutations, 0);
        assert!(background.fair_mutations.is_empty());
    }

    #[test]
    fn background_worker_inherited_fair_nice_is_restored_to_startup_nice() {
        let mut background = FakeOps::normal(-10);

        let snapshot = configure_role(
            SchedulingThreadRole::Background,
            SchedulingQosPolicy::Auto,
            50,
            0,
            &mut background,
        )
        .unwrap();

        assert_eq!(
            snapshot.effective_class,
            EffectiveSchedulingClass::Fair { nice: 0 }
        );
        assert_eq!(
            snapshot.requested_class,
            SchedulingClass::Background { nice: 0 }
        );
        assert_eq!(snapshot.effective_nice, 0);
        assert_eq!(background.normal_mutations, 1);
        assert!(background.fair_mutations.is_empty());
    }

    #[test]
    fn kms_inherited_realtime_is_demoted_before_worker_operation() {
        let mut kms = FakeOps::normal(0);
        kms.state.policy = PolicyKind::RoundRobin;
        kms.state.priority = REACTOR_RR_PRIORITY;
        kms.state.reset_on_fork = true;

        let snapshot = configure_role(
            SchedulingThreadRole::KmsCommit,
            SchedulingQosPolicy::Auto,
            51,
            0,
            &mut kms,
        )
        .unwrap();

        assert_eq!(kms.rr_mutations, 0);
        assert_eq!(
            snapshot.effective_class,
            EffectiveSchedulingClass::Fair { nice: 0 }
        );
    }

    #[test]
    fn kms_fair_scheduling_failure_keeps_normal_worker_operation() {
        let mut kms = FakeOps::normal(0);
        kms.direct_fair_result = Err("EPERM");
        kms.rtkit_result = Err("RTKit unavailable");
        let snapshot = configure_role(
            SchedulingThreadRole::KmsCommit,
            SchedulingQosPolicy::Auto,
            52,
            0,
            &mut kms,
        )
        .expect("KMS QoS failure must not stop the worker");

        assert_eq!(snapshot.backend, SchedulingBackend::None);
        assert_eq!(
            snapshot.effective_class,
            EffectiveSchedulingClass::Fair { nice: 0 }
        );
        assert!(snapshot.degradation_reason.is_some());
        assert_eq!(kms.rr_mutations, 0);
    }

    #[test]
    fn reactor_realtime_request_includes_reset_on_fork() {
        let mut ops = FakeOps::normal(0);
        let snapshot = configure_role(
            SchedulingThreadRole::NativeReactor,
            SchedulingQosPolicy::Auto,
            50,
            0,
            &mut ops,
        )
        .expect("realtime request is established");

        assert!(snapshot.reset_on_fork_effective);
        assert!(
            EffectiveSchedulingClass::RealtimeRoundRobin { priority: 1 }
                .is_expected_reactor_rr(snapshot.reset_on_fork_effective)
        );
    }

    #[test]
    fn realtime_demotion_and_budget_overrun_are_visible() {
        let mut ops = FakeOps::normal(0);
        ops.rr_result = Err("EPERM");
        ops.overrun = true;
        ops.demotions = 1;
        let snapshot = configure_role(
            SchedulingThreadRole::NativeReactor,
            SchedulingQosPolicy::Auto,
            51,
            0,
            &mut ops,
        )
        .expect("auto mode degrades");

        assert_eq!(snapshot.demotion_count, 1);
        assert!(snapshot.rt_budget_overrun);
    }

    #[test]
    fn helper_creation_blocks_realtime_inheritance_and_normalizes_worker_nice() {
        let rt_creator = KernelSchedulingState {
            policy: PolicyKind::RoundRobin,
            priority: 1,
            nice: 0,
            reset_on_fork: true,
        };
        let reniced_creator = KernelSchedulingState {
            policy: PolicyKind::Other,
            priority: 0,
            nice: -10,
            reset_on_fork: false,
        };
        assert_eq!(
            normalized_spawn_creator_state(rt_creator, 0),
            KernelSchedulingState {
                policy: PolicyKind::Other,
                priority: 0,
                nice: 0,
                reset_on_fork: false,
            }
        );
        assert_eq!(
            normalized_spawn_creator_state(reniced_creator, 0),
            reniced_creator
        );

        let source = include_str!("scheduling_qos.rs");
        let function = source
            .split_once("fn with_normalized_thread_creator")
            .expect("thread creator guard exists")
            .1;
        let normalize = function
            .find("normalized_spawn_creator_state")
            .expect("creator state is normalized");
        let normal_policy = function
            .find("set_scheduler_other(tid)")
            .expect("creator policy is ordinary before spawn");
        let normal_nice = function
            .find("set_nice(tid, desired.nice)")
            .expect("creator nice is ordinary before spawn");
        let spawn = function
            .find("let spawned = spawn()")
            .expect("classified child is spawned");
        assert!(normalize < spawn);
        assert!(normal_policy < spawn);
        assert!(normal_nice < spawn);

        let thread_start = source
            .split_once("thread::Builder::new().name(name.to_owned()).spawn(move || {")
            .expect("classified worker trampoline exists")
            .1;
        let child_nice = thread_start
            .find("set_nice(tid, manager.baseline_nice)")
            .expect("worker resets inherited nice");
        let role_work = thread_start
            .find("register_current_thread_role(role)")
            .expect("role registration precedes generic work");
        let generic_work = thread_start
            .find("work()")
            .expect("generic worker closure begins");
        assert!(child_nice < role_work && role_work < generic_work);
    }

    #[test]
    fn audited_production_helpers_do_not_spawn_raw_threads() {
        let sources = [
            include_str!("../application_scope.rs"),
            include_str!("../compositor/pacing.rs"),
            include_str!("../cursor_manager.rs"),
            include_str!("../keyboard_persistence.rs"),
            include_str!("dmem_foreground.rs"),
            include_str!("../native_output/kms_worker/thread.rs"),
            include_str!("../native_output/pacing.rs"),
            include_str!("../native_output/presentation/trace_writer.rs"),
            include_str!("../native_output/runtime/output_configuration.rs"),
        ];
        for source in sources {
            assert!(!source.contains("thread::spawn("));
            assert!(!source.contains("thread::Builder::new()"));
            assert!(!source.contains("std::thread::spawn("));
        }

        let application_scope = include_str!("../application_scope.rs");
        assert!(application_scope.contains("scheduling_qos::spawn_background"));
        let kms_worker = include_str!("../native_output/kms_worker/thread.rs");
        assert!(kms_worker.contains("scheduling_qos::spawn_role_thread"));
        assert!(kms_worker.contains("SchedulingThreadRole::KmsCommit"));
    }

    #[test]
    fn signal_handler_is_confined_to_async_safe_demote_operations() {
        let source = include_str!("scheduling_qos.rs");
        let start = source
            .find("extern \"C\" fn sigxcpu_demote")
            .expect("SIGXCPU handler exists");
        let body_start = source[start..]
            .find('{')
            .map(|offset| start + offset + 1)
            .expect("SIGXCPU handler body begins");
        let mut depth = 1usize;
        let mut body_end = body_start;
        for (offset, byte) in source.as_bytes()[body_start..].iter().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        body_end = body_start + offset;
                        break;
                    }
                }
                _ => {}
            }
        }
        let body = &source[body_start..body_end];
        for forbidden in [
            "format!(",
            "println!(",
            "eprintln!(",
            "Vec::",
            "String::",
            "Mutex",
            ".lock()",
            "std::fs",
            "make_thread_high_priority(",
        ] {
            assert!(
                !body.contains(forbidden),
                "signal handler contains {forbidden}"
            );
        }
        assert!(body.contains("SYS_sched_setscheduler"));
        assert!(body.contains("RT_BUDGET_OVERRUN.store"));
    }

    #[test]
    fn signal_budget_and_dbus_setup_stay_outside_runtime_cycles() {
        let cycle = include_str!("../native_output/runtime/cycle.rs");
        let run_native_cycle = cycle
            .split_once("pub(super) fn run_native_cycle")
            .expect("native cycle entry exists")
            .1;
        let promote = run_native_cycle
            .find("promote_current_reactor()")
            .expect("reactor promotes at lifecycle boundary");
        let loop_start = run_native_cycle
            .find("while !self.shutdown.is_complete()")
            .expect("cycle loop exists");
        assert!(promote < loop_start);
        let run_cycle = cycle
            .split_once("fn run_cycle(&mut self)")
            .expect("per-cycle function exists")
            .1;
        assert!(!run_cycle.contains("zbus::"));
        assert!(!run_cycle.contains("make_thread_high_priority("));

        let process = include_str!("../process.rs");
        let signalfd = process
            .split_once("fn create_runtime_signalfd()")
            .expect("runtime signalfd setup exists")
            .1;
        assert!(!signalfd.contains("SIGXCPU"));
        assert!(signalfd.contains("SIGCHLD"));
        assert!(signalfd.contains("SIGTERM"));
        let qos = include_str!("scheduling_qos.rs");
        assert!(qos.contains("pthread_sigmask(libc::SIG_UNBLOCK"));
        assert!(qos.contains("libc::SIG_SETMASK"));

        let presentation = include_str!("../native_output/runtime/presentation_cycle.rs");
        assert!(presentation.contains("queue_primary_geometry_test_if_idle"));
        assert!(!presentation.contains("test_primary_geometry_if_idle"));
        let kms_worker = include_str!("../native_output/kms_worker/thread.rs");
        let probe_method = kms_worker
            .split_once("pub(crate) fn queue_primary_geometry_test_if_idle")
            .expect("scaled probe queues on the bounded KMS lane")
            .1
            .split_once("pub(crate) fn cancel_queued_primary")
            .expect("probe method ends before ordinary queue operations")
            .0;
        assert!(!probe_method.contains(".recv()"));
    }

    #[test]
    fn diagnostic_snapshot_registry_never_waits_for_a_worker_lock() {
        let registry = Mutex::new(BTreeMap::new());
        let _held = registry.lock().unwrap();

        assert!(try_clone_snapshot_registry(&registry).is_none());
    }

    #[test]
    fn managed_child_launches_restore_startup_nice_and_rt_limit() {
        let qos = include_str!("scheduling_qos.rs");
        let child_setup = qos
            .split_once("pub(crate) fn prepare_child_scheduling")
            .expect("child QoS restoration hook exists")
            .1;
        assert!(child_setup.contains("command.pre_exec"));
        assert!(child_setup.contains("setrlimit(libc::RLIMIT_RTTIME"));
        assert!(child_setup.contains("setpriority(libc::PRIO_PROCESS, 0, nice)"));
        assert!(child_setup.contains("PR_CAP_AMBIENT_CLEAR_ALL"));

        let process = include_str!("../process.rs");
        let child_supervisor = process
            .split_once("fn spawn_inner(")
            .expect("all managed children use spawn_inner")
            .1;
        assert!(child_supervisor.contains("prepare_child_scheduling(command)"));
    }
}
