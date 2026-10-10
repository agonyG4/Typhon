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
// Linux tracks RLIMIT_RTTIME in scheduler ticks and compares their converted
// microsecond count with the soft and hard thresholds. The kernel checks hard
// expiration before sending soft-limit SIGXCPU, so reserve at least two ticks
// at generic HZ=100 (10ms each), then reserve 25% of longer finite limits for
// signal delivery, scheduling, and the demotion syscall.
const RTTIME_MIN_WARNING_HEADROOM_US: u64 = 20_000;
const RTTIME_HARD_HEADROOM_NUMERATOR: u64 = 1;
const RTTIME_HARD_HEADROOM_DENOMINATOR: u64 = 4;

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
    fn request_rtkit_high_priority_fair(&mut self, tid: i32, nice: i32) -> Result<(), String>;
    fn request_direct_high_priority_fair(&mut self, tid: i32, nice: i32) -> Result<(), String>;
    fn request_normal_fair(&mut self, tid: i32, nice: i32) -> Result<(), String>;
    fn demotion_count(&self, tid: i32) -> u64;
    fn rt_budget_overrun(&self, tid: i32) -> bool;
}

trait ReactorSchedulingOperations: SchedulingOperations {
    fn prepare_realtime_safety(&mut self, tid: i32) -> Result<(), String>;
    fn restore_realtime_safety(&mut self, tid: i32);
    fn request_realtime_round_robin(
        &mut self,
        tid: i32,
        priority: i32,
        reset_on_fork: bool,
    ) -> Result<(), String>;
}

#[cfg(test)]
fn configure_role(
    role: SchedulingThreadRole,
    mode: SchedulingQosPolicy,
    tid: i32,
    baseline_nice: i32,
    ops: &mut impl ReactorSchedulingOperations,
) -> Result<SchedulingDiagnosticSnapshot, RequiredReactorPolicyError> {
    configure_role_with_rtkit(role, mode, tid, baseline_nice, true, ops)
}

#[cfg(test)]
fn configure_role_with_rtkit(
    role: SchedulingThreadRole,
    mode: SchedulingQosPolicy,
    tid: i32,
    baseline_nice: i32,
    allow_reactor_rtkit: bool,
    ops: &mut impl ReactorSchedulingOperations,
) -> Result<SchedulingDiagnosticSnapshot, RequiredReactorPolicyError> {
    if role == SchedulingThreadRole::NativeReactor {
        configure_reactor_policy_with_rtkit(mode, tid, baseline_nice, allow_reactor_rtkit, ops)
    } else {
        Ok(configure_worker_policy(role, mode, tid, baseline_nice, ops))
    }
}

fn initial_kernel_state(
    tid: i32,
    baseline_nice: i32,
    ops: &mut impl SchedulingOperations,
    degradation: &mut Vec<String>,
) -> KernelSchedulingState {
    match ops.inspect(tid) {
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
    }
}

fn configure_worker_policy(
    role: SchedulingThreadRole,
    mode: SchedulingQosPolicy,
    tid: i32,
    baseline_nice: i32,
    ops: &mut impl SchedulingOperations,
) -> SchedulingDiagnosticSnapshot {
    assert_ne!(
        role,
        SchedulingThreadRole::NativeReactor,
        "native reactor scheduling requires reactor-only authority"
    );
    let requested_class = if mode == SchedulingQosPolicy::Off {
        SchedulingClass::Normal
    } else {
        SchedulingClass::requested_for(role, baseline_nice)
    };
    let mut degradation = Vec::new();
    let mut backend = SchedulingBackend::Inherited;
    let mut effective = initial_kernel_state(tid, baseline_nice, ops, &mut degradation);

    if mode == SchedulingQosPolicy::Off {
        return snapshot(role, tid, requested_class, effective, backend, ops, None);
    }

    match role {
        SchedulingThreadRole::KmsCommit | SchedulingThreadRole::Render => {
            if EffectiveSchedulingClass::from_kernel(effective)
                .is_fair_priority_above(baseline_nice)
            {
                return snapshot(role, tid, requested_class, effective, backend, ops, None);
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
        SchedulingThreadRole::NativeReactor => unreachable!(),
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

    snapshot(
        role,
        tid,
        requested_class,
        effective,
        backend,
        ops,
        degradation_reason,
    )
}

fn configure_reactor_policy(
    mode: SchedulingQosPolicy,
    tid: i32,
    baseline_nice: i32,
    ops: &mut impl ReactorSchedulingOperations,
) -> Result<SchedulingDiagnosticSnapshot, RequiredReactorPolicyError> {
    configure_reactor_policy_with_rtkit(mode, tid, baseline_nice, true, ops)
}

fn configure_reactor_policy_with_rtkit(
    mode: SchedulingQosPolicy,
    tid: i32,
    baseline_nice: i32,
    allow_reactor_rtkit: bool,
    ops: &mut impl ReactorSchedulingOperations,
) -> Result<SchedulingDiagnosticSnapshot, RequiredReactorPolicyError> {
    let role = SchedulingThreadRole::NativeReactor;
    let requested_class = if mode == SchedulingQosPolicy::Off {
        SchedulingClass::Normal
    } else {
        SchedulingClass::requested_for(role, baseline_nice)
    };
    let mut degradation = Vec::new();
    let mut effective = initial_kernel_state(tid, baseline_nice, ops, &mut degradation);
    let mut backend = SchedulingBackend::Inherited;
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

    let inherited_rr = EffectiveSchedulingClass::from_kernel(effective)
        .is_expected_reactor_rr(effective.reset_on_fork);
    let mut rr_is_verified = false;
    let mut safety_is_active = false;
    if inherited_rr {
        match ops.prepare_realtime_safety(tid) {
            Ok(()) => {
                safety_is_active = true;
                rr_is_verified = true;
                backend = SchedulingBackend::Inherited;
            }
            Err(error) => {
                degradation.push(format!("inherited SCHED_RR safety setup failed: {error}"));
                normalize_reactor_for_fair_fallback(
                    tid,
                    baseline_nice,
                    &mut effective,
                    &mut degradation,
                    &mut safety_is_active,
                    ops,
                );
            }
        }
    } else {
        match ops.prepare_realtime_safety(tid) {
            Ok(()) => {
                safety_is_active = true;
                match ops.request_realtime_round_robin(tid, REACTOR_RR_PRIORITY, true) {
                    Ok(()) => match ops.inspect(tid) {
                        Ok(actual)
                            if EffectiveSchedulingClass::from_kernel(actual)
                                .is_expected_reactor_rr(actual.reset_on_fork) =>
                        {
                            effective = actual;
                            backend = SchedulingBackend::Direct;
                            rr_is_verified = true;
                        }
                        Ok(actual) => {
                            degradation.push(format!(
                                "SCHED_RR request did not verify (policy={} priority={} reset_on_fork={})",
                                EffectiveSchedulingClass::from_kernel(actual).as_str(),
                                actual.priority,
                                actual.reset_on_fork
                            ));
                            effective = actual;
                        }
                        Err(error) => {
                            degradation.push(format!("SCHED_RR verification failed: {error}"))
                        }
                    },
                    Err(error) => degradation.push(format!("direct SCHED_RR failed: {error}")),
                }
            }
            Err(error) => degradation.push(format!("direct SCHED_RR safety setup failed: {error}")),
        }
        if !rr_is_verified && safety_is_active {
            normalize_reactor_for_fair_fallback(
                tid,
                baseline_nice,
                &mut effective,
                &mut degradation,
                &mut safety_is_active,
                ops,
            );
        }
    }

    if rr_is_verified {
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

    let mut fallback_state = SchedulingFallbackState {
        effective: &mut effective,
        backend: &mut backend,
        degradation: &mut degradation,
    };
    let fair_policy_acquired = if allow_reactor_rtkit {
        request_and_verify_fair(
            ops,
            tid,
            HIGH_PRIORITY_NICE,
            baseline_nice,
            true,
            &mut fallback_state,
        )
    } else {
        request_and_verify_direct_fair(
            ops,
            tid,
            HIGH_PRIORITY_NICE,
            baseline_nice,
            &mut fallback_state,
        )
    };
    if fair_policy_acquired {
        restore_reactor_safety_if_ordinary(tid, effective, &mut safety_is_active, ops);
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

    if !matches!(effective.policy, PolicyKind::Other) || effective.nice != baseline_nice {
        let _ = ops.request_normal_fair(tid, baseline_nice);
        if let Ok(actual) = ops.inspect(tid) {
            effective = actual;
        }
    }
    restore_reactor_safety_if_ordinary(tid, effective, &mut safety_is_active, ops);
    backend = SchedulingBackend::None;
    Ok(snapshot(
        role,
        tid,
        requested_class,
        effective,
        backend,
        ops,
        Some(join_reasons(&degradation, "using ordinary SCHED_OTHER")),
    ))
}

fn normalize_reactor_for_fair_fallback(
    tid: i32,
    baseline_nice: i32,
    effective: &mut KernelSchedulingState,
    degradation: &mut Vec<String>,
    safety_is_active: &mut bool,
    ops: &mut impl ReactorSchedulingOperations,
) {
    if let Err(error) = ops.request_normal_fair(tid, baseline_nice) {
        degradation.push(format!(
            "failed to normalize reactor before FAIR fallback: {error}"
        ));
    }
    match ops.inspect(tid) {
        Ok(actual) => *effective = actual,
        Err(error) => degradation.push(format!("reactor normalization query failed: {error}")),
    }
    if matches!(effective.policy, PolicyKind::Other) {
        restore_reactor_safety_if_ordinary(tid, *effective, safety_is_active, ops);
    } else if *safety_is_active {
        degradation.push(
            "realtime safety remains active because reactor could not be normalized".to_owned(),
        );
    }
}

fn restore_reactor_safety_if_ordinary(
    tid: i32,
    effective: KernelSchedulingState,
    safety_is_active: &mut bool,
    ops: &mut impl ReactorSchedulingOperations,
) {
    if *safety_is_active && matches!(effective.policy, PolicyKind::Other) {
        ops.restore_realtime_safety(tid);
        *safety_is_active = false;
    }
}

fn prepare_reactor_for_session_suspend(
    tid: i32,
    baseline_nice: i32,
    ops: &mut impl ReactorSchedulingOperations,
) -> Result<(), String> {
    let mut state = ops.inspect(tid)?;
    if matches!(
        state.policy,
        PolicyKind::RoundRobin | PolicyKind::Fifo | PolicyKind::Deadline
    ) {
        let request_error = ops.request_normal_fair(tid, baseline_nice).err();
        state = ops.inspect(tid)?;
        if !matches!(state.policy, PolicyKind::Other) {
            return Err(format!(
                "reactor remained {} at session suspend boundary (normalization: {})",
                EffectiveSchedulingClass::from_kernel(state).as_str(),
                request_error.unwrap_or_else(|| "request reported success".to_owned())
            ));
        }
    }
    if matches!(state.policy, PolicyKind::Other) {
        // FAIR priority is safe across a potentially blocking DRM/session
        // boundary. An RR budget is process-scoped and belongs only to this
        // reactor, so retire it once RR has been verified absent.
        ops.restore_realtime_safety(tid);
        return Ok(());
    }
    Err("reactor scheduling state is unknown at session suspend boundary".to_owned())
}

fn reacquire_reactor_policy_after_suspend(
    mode: SchedulingQosPolicy,
    tid: i32,
    baseline_nice: i32,
    overrun: bool,
    previous: Option<&SchedulingDiagnosticSnapshot>,
    ops: &mut impl ReactorSchedulingOperations,
) -> Result<SchedulingDiagnosticSnapshot, RequiredReactorPolicyError> {
    let role = SchedulingThreadRole::NativeReactor;
    let requested_class = if mode == SchedulingQosPolicy::Off {
        SchedulingClass::Normal
    } else {
        SchedulingClass::requested_for(role, baseline_nice)
    };
    let mut degradation = Vec::new();
    let mut effective = initial_kernel_state(tid, baseline_nice, ops, &mut degradation);

    if overrun {
        if matches!(
            effective.policy,
            PolicyKind::RoundRobin | PolicyKind::Fifo | PolicyKind::Deadline
        ) {
            let _ = ops.request_normal_fair(tid, baseline_nice);
            match ops.inspect(tid) {
                Ok(actual) => effective = actual,
                Err(error) => {
                    degradation.push(format!("post-overrun normalization failed: {error}"))
                }
            }
        }
        if matches!(effective.policy, PolicyKind::Other) {
            ops.restore_realtime_safety(tid);
        }
        return Ok(snapshot(
            role,
            tid,
            requested_class,
            effective,
            SchedulingBackend::None,
            ops,
            Some(join_reasons(
                &degradation,
                "RT budget overrun; realtime reacquisition suppressed",
            )),
        ));
    }

    if matches!(effective.policy, PolicyKind::Other) && effective.nice < baseline_nice {
        return Ok(snapshot(
            role,
            tid,
            previous
                .map(|snapshot| snapshot.requested_class)
                .unwrap_or(requested_class),
            effective,
            previous
                .map(|snapshot| snapshot.backend)
                .unwrap_or(SchedulingBackend::Inherited),
            ops,
            previous.and_then(|snapshot| snapshot.degradation_reason.clone()),
        ));
    }

    configure_reactor_policy_with_rtkit(mode, tid, baseline_nice, false, ops)
}

fn prepare_reactor_for_shutdown(
    tid: i32,
    baseline_nice: i32,
    ops: &mut impl ReactorSchedulingOperations,
) -> Result<(), String> {
    let request_result = ops.request_normal_fair(tid, baseline_nice);
    let state = ops.inspect(tid)?;
    if matches!(state.policy, PolicyKind::Other) {
        ops.restore_realtime_safety(tid);
    }
    if !matches!(state.policy, PolicyKind::Other) || state.nice != baseline_nice {
        return Err(format!(
            "shutdown policy did not normalize to SCHED_OTHER nice {baseline_nice} (policy={} nice={}, request={})",
            EffectiveSchedulingClass::from_kernel(state).as_str(),
            state.nice,
            request_result
                .err()
                .unwrap_or_else(|| "verified".to_owned())
        ));
    }
    Ok(())
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
        reset_on_fork: creator.reset_on_fork,
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum RtTimeLimitSafetyError {
    ZeroSoftBudget,
    HardLimitTooSmall {
        hard_us: u64,
        required_headroom_us: u64,
    },
    SoftBudgetOutOfRange,
}

impl fmt::Display for RtTimeLimitSafetyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroSoftBudget => {
                formatter.write_str("RLIMIT_RTTIME soft budget is zero; realtime disabled")
            }
            Self::HardLimitTooSmall {
                hard_us,
                required_headroom_us,
            } => write!(
                formatter,
                "RLIMIT_RTTIME hard limit {hard_us}us cannot provide {required_headroom_us}us of SIGXCPU warning headroom; realtime disabled"
            ),
            Self::SoftBudgetOutOfRange => {
                formatter.write_str("RLIMIT_RTTIME soft budget is out of range; realtime disabled")
            }
        }
    }
}
const BROKER_QUEUE_CAPACITY: usize = 8;
const BROKER_REPLY_TIMEOUT: Duration = Duration::from_millis(750);

static MANAGER: OnceLock<QosManager> = OnceLock::new();
static ACTIVE_REACTOR_TID: AtomicI32 = AtomicI32::new(0);
static RT_BUDGET_REACTOR_TID: AtomicI32 = AtomicI32::new(0);
static RT_BUDGET_OVERRUN: AtomicBool = AtomicBool::new(false);
static RT_DEMOTION_COUNT: AtomicU64 = AtomicU64::new(0);
static SIGXCPU_HANDLER_INSTALLED: AtomicBool = AtomicBool::new(false);
const SIGXCPU_DEMOTION_POLICY: libc::c_int = libc::SCHED_OTHER | libc::SCHED_RESET_ON_FORK;

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
    if manager.policy != SchedulingQosPolicy::Off {
        crate::compositor::pacing::prime_enabled_trace_sinks();
    }
    // Take the registry while the current thread is still ordinary so
    // publishing verified RR state cannot block behind a lower-priority worker.
    let mut snapshots = manager
        .snapshots
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut operations = LinuxSchedulingOperations { manager };
    let configured =
        configure_reactor_policy(manager.policy, tid, manager.baseline_nice, &mut operations);
    match configured {
        Ok(snapshot) => {
            if !matches!(
                snapshot.effective_class,
                EffectiveSchedulingClass::RealtimeRoundRobin {
                    priority: REACTOR_RR_PRIORITY
                }
            ) || !snapshot.reset_on_fork_effective
            {
                manager.restore_rt_budget_for_reactor(tid);
            }
            snapshots.insert((snapshot.role, snapshot.tid), snapshot.clone());
            Ok(snapshot)
        }
        Err(error) => {
            if manager.policy != SchedulingQosPolicy::Off {
                let tid = unsafe { libc::gettid() };
                let _ = set_scheduler_other_preserving_flags(tid);
                let _ = set_nice(tid, manager.baseline_nice);
                if matches!(inspect_kernel_state(tid), Ok(state) if matches!(state.policy, PolicyKind::Other))
                {
                    manager.restore_rt_budget_for_reactor(tid);
                }
            }
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
    let previous = snapshots
        .get(&(SchedulingThreadRole::NativeReactor, tid))
        .cloned();
    let snapshot = reacquire_reactor_policy_after_suspend(
        mode,
        tid,
        manager.baseline_nice,
        RT_BUDGET_OVERRUN.load(Ordering::Acquire),
        previous.as_ref(),
        &mut operations,
    )
    .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)?;
    if !snapshot
        .effective_class
        .is_expected_reactor_rr(snapshot.reset_on_fork_effective)
    {
        manager.restore_rt_budget_for_reactor(tid);
    }
    snapshots.insert((snapshot.role, snapshot.tid), snapshot.clone());
    Ok(snapshot)
}

/// Retain safe FAIR priority over a session boundary while removing RR.
pub fn prepare_current_reactor_for_session_suspend() -> Result<(), String> {
    let Some(manager) = MANAGER.get() else {
        return Ok(());
    };
    if manager.policy == SchedulingQosPolicy::Off {
        return Ok(());
    }
    let tid = unsafe { libc::gettid() };
    let mut operations = LinuxSchedulingOperations { manager };
    let previous = try_reactor_snapshot(manager, tid);
    prepare_reactor_for_session_suspend(tid, manager.baseline_nice, &mut operations)?;
    update_reactor_snapshot(manager, tid, previous);
    Ok(())
}

/// Normalize elevated FAIR priority and remove the RT budget before shutdown joins.
pub fn demote_current_reactor() {
    let Some(manager) = MANAGER.get() else {
        return;
    };
    if manager.policy == SchedulingQosPolicy::Off {
        return;
    }
    let tid = unsafe { libc::gettid() };
    let mut operations = LinuxSchedulingOperations { manager };
    let previous = try_reactor_snapshot(manager, tid);
    if let Err(error) = prepare_reactor_for_shutdown(tid, manager.baseline_nice, &mut operations) {
        eprintln!("scheduling QoS: reactor shutdown normalization failed: {error}");
    }
    update_reactor_snapshot(manager, tid, previous);
}

fn update_reactor_snapshot(
    manager: &QosManager,
    tid: i32,
    previous: Option<SchedulingDiagnosticSnapshot>,
) {
    if let (Some(mut snapshot), Ok(state)) = (previous, inspect_kernel_state(tid)) {
        snapshot.effective_class = EffectiveSchedulingClass::from_kernel(state);
        snapshot.effective_priority = state.priority;
        snapshot.effective_nice = state.nice;
        snapshot.reset_on_fork_effective = state.reset_on_fork;
        snapshot.demotion_count = RT_DEMOTION_COUNT.load(Ordering::Relaxed);
        snapshot.rt_budget_overrun = RT_BUDGET_OVERRUN.load(Ordering::Relaxed);
        manager.record(snapshot);
    }
}

fn try_reactor_snapshot(manager: &QosManager, tid: i32) -> Option<SchedulingDiagnosticSnapshot> {
    match manager.snapshots.try_lock() {
        Ok(snapshots) => snapshots
            .get(&(SchedulingThreadRole::NativeReactor, tid))
            .cloned(),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned
            .into_inner()
            .get(&(SchedulingThreadRole::NativeReactor, tid))
            .cloned(),
        Err(std::sync::TryLockError::WouldBlock) => None,
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
    manager.restore_rt_budget_for_reactor(current_tid().unwrap_or(0));
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
    assert_ne!(
        role,
        SchedulingThreadRole::NativeReactor,
        "reactor must use reactor-only scheduling authority"
    );
    let tid = unsafe { libc::gettid() };
    if let Some(manager) = MANAGER.get() {
        let mut operations = LinuxSchedulingOperations { manager };
        let snapshot = configure_worker_policy(
            role,
            manager.policy,
            tid,
            manager.baseline_nice,
            &mut operations,
        );
        manager.record(snapshot);
    }
    RoleRegistration { role, tid }
}

/// Create a classified helper. A realtime creator is temporarily normalized
/// before pthread creation so the child inherits SCHED_OTHER before Rust
/// starts it. The child trampoline normalizes inherited nice before role work.
///
/// When called by the reactor after promotion, this briefly removes its RR
/// starvation protection while pthread creation runs. Runtime call sites must
/// therefore be limited to optional lazy helpers; latency-relevant workers
/// should be created during bootstrap or while the reactor is demoted.
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
    if role == SchedulingThreadRole::NativeReactor {
        return Err(io::Error::other(
            "the native reactor must be initialized by its runtime owner, not spawned as a helper",
        ));
    }
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
        set_scheduler_other_preserving_flags(tid).map_err(io::Error::other)?;
    }
    if nice_changed && let Err(error) = set_nice(tid, desired.nice) {
        let sigxcpu_demoted = can_restore_rt && ACTIVE_REACTOR_TID.load(Ordering::Acquire) != tid;
        if scheduler_changed {
            if !sigxcpu_demoted {
                let _ = restore_thread_scheduler(tid, prior);
            }
        }
        if can_restore_rt && ACTIVE_REACTOR_TID.load(Ordering::Acquire) != tid {
            let _ = set_scheduler_other_preserving_flags(tid);
            let _ = set_nice(tid, manager.baseline_nice);
            manager.restore_rt_budget_for_reactor(tid);
            record_reactor_degradation(
                manager,
                tid,
                "SIGXCPU demoted reactor during normalized helper creation",
            );
        }
        return Err(io::Error::other(error));
    }

    let spawned = spawn();
    if can_restore_rt && ACTIVE_REACTOR_TID.load(Ordering::Acquire) != tid {
        manager.restore_rt_budget_for_reactor(tid);
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
        let _ = set_scheduler_other_preserving_flags(tid);
        let _ = set_nice(tid, manager.baseline_nice);
        if can_restore_rt {
            manager.restore_rt_budget_for_reactor(tid);
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
        let _ = set_scheduler_other_preserving_flags(tid);
        let _ = set_nice(tid, manager.baseline_nice);
        manager.restore_rt_budget_for_reactor(tid);
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
    let effective_policy = unsafe { libc::sched_getscheduler(tid) };
    if effective_policy < 0 {
        return Err(io::Error::last_os_error());
    }
    if effective_policy != policy {
        return Err(io::Error::other(format!(
            "scheduler restoration requested policy {policy:#x}, observed {effective_policy:#x}"
        )));
    }
    let mut effective_parameter = libc::sched_param { sched_priority: -1 };
    if unsafe { libc::sched_getparam(tid, &mut effective_parameter) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if effective_parameter.sched_priority != state.priority {
        return Err(io::Error::other(format!(
            "scheduler restoration requested priority {}, observed {}",
            state.priority, effective_parameter.sched_priority
        )));
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
        if RT_BUDGET_OVERRUN.load(Ordering::Acquire) {
            return Err("previous RT budget overrun permanently disabled realtime".to_owned());
        }
        if self.budget_active.load(Ordering::Acquire) {
            if RT_BUDGET_REACTOR_TID.load(Ordering::Acquire) == tid
                && !RT_BUDGET_OVERRUN.load(Ordering::Acquire)
                && ACTIVE_REACTOR_TID.load(Ordering::Acquire) == tid
            {
                return Ok(());
            }
            return Err("realtime safety budget is owned by another or demoted reactor".to_owned());
        }
        let original_rt_limit = self
            .original_rt_limit
            .ok_or_else(|| "RLIMIT_RTTIME could not be queried; realtime disabled".to_owned())?;
        let guarded =
            guarded_rt_limit(original_rt_limit, RT_BUDGET_US).map_err(|e| e.to_string())?;
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

        // SIGXCPU is process-directed. Keep Typhon's demotion action installed
        // for the process lifetime so a pending group signal cannot observe a
        // restored default disposition during teardown.
        if let Err(error) = install_sigxcpu_handler() {
            unsafe {
                libc::pthread_sigmask(libc::SIG_SETMASK, &previous_mask, std::ptr::null_mut());
            }
            return Err(error);
        }
        if unsafe { libc::setrlimit(libc::RLIMIT_RTTIME, &guarded) } != 0 {
            let error = io::Error::last_os_error();
            unsafe {
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
                    libc::pthread_sigmask(libc::SIG_SETMASK, &previous_mask, std::ptr::null_mut());
                }
                return Err("SIGXCPU signal-mask state is poisoned".to_owned());
            }
        }
        ACTIVE_REACTOR_TID.store(tid, Ordering::SeqCst);
        RT_BUDGET_REACTOR_TID.store(tid, Ordering::SeqCst);
        self.budget_active.store(true, Ordering::Release);
        let unblock_result =
            unsafe { libc::pthread_sigmask(libc::SIG_UNBLOCK, &signal_set, std::ptr::null_mut()) };
        if unblock_result != 0 {
            self.restore_rt_budget_for_reactor(tid);
            return Err(format!(
                "cannot unblock SIGXCPU for the realtime reactor: {}",
                io::Error::from_raw_os_error(unblock_result)
            ));
        }
        Ok(())
    }

    fn restore_rt_budget_for_reactor(&self, tid: i32) {
        if tid <= 0 || RT_BUDGET_REACTOR_TID.load(Ordering::Acquire) != tid {
            return;
        }
        let _ = ACTIVE_REACTOR_TID.compare_exchange(tid, 0, Ordering::SeqCst, Ordering::Relaxed);
        if !self.budget_active.swap(false, Ordering::AcqRel) {
            return;
        }
        RT_BUDGET_REACTOR_TID.store(0, Ordering::SeqCst);
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
        if let Ok(mut saved_mask) = self.saved_reactor_signal_mask.lock()
            && let Some(previous_mask) = saved_mask.take()
        {
            // The fail-safe disposition remains installed. If SIGXCPU became
            // pending while blocked, its handler now sees no active reactor.
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

fn install_sigxcpu_handler() -> Result<(), String> {
    if SIGXCPU_HANDLER_INSTALLED.load(Ordering::Acquire) {
        return Ok(());
    }
    let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
    action.sa_sigaction = sigxcpu_demote as *const () as usize;
    action.sa_flags = libc::SA_RESTART;
    unsafe { libc::sigemptyset(&mut action.sa_mask) };
    // SAFETY: The handler only performs lock-free atomic operations and a
    // direct scheduler syscall. SIGXCPU is reserved for this safety mechanism:
    // its process-level disposition remains installed, and an inactive reactor
    // makes the handler return without action. It is not owned by signalfd.
    if unsafe { libc::sigaction(libc::SIGXCPU, &action, std::ptr::null_mut()) } != 0 {
        return Err(format!(
            "cannot install SIGXCPU fail-safe: {}",
            io::Error::last_os_error()
        ));
    }
    SIGXCPU_HANDLER_INSTALLED.store(true, Ordering::Release);
    Ok(())
}

struct LinuxSchedulingOperations<'a> {
    manager: &'a QosManager,
}

impl SchedulingOperations for LinuxSchedulingOperations<'_> {
    fn inspect(&mut self, tid: i32) -> Result<KernelSchedulingState, String> {
        inspect_kernel_state(tid)
    }

    fn request_rtkit_high_priority_fair(&mut self, tid: i32, nice: i32) -> Result<(), String> {
        set_scheduler_other_preserving_flags(tid)?;
        set_nice(tid, self.manager.baseline_nice)?;
        self.manager.request_rtkit_high_priority_fair(tid, nice)
    }

    fn request_direct_high_priority_fair(&mut self, tid: i32, nice: i32) -> Result<(), String> {
        set_scheduler_other_preserving_flags(tid)?;
        set_nice(tid, nice)
    }

    fn request_normal_fair(&mut self, tid: i32, nice: i32) -> Result<(), String> {
        set_scheduler_other_preserving_flags(tid)?;
        set_nice(tid, nice)
    }

    fn demotion_count(&self, _tid: i32) -> u64 {
        RT_DEMOTION_COUNT.load(Ordering::Relaxed)
    }

    fn rt_budget_overrun(&self, _tid: i32) -> bool {
        RT_BUDGET_OVERRUN.load(Ordering::Relaxed)
    }
}

impl ReactorSchedulingOperations for LinuxSchedulingOperations<'_> {
    fn prepare_realtime_safety(&mut self, tid: i32) -> Result<(), String> {
        self.manager.prepare_rt_budget(tid)
    }

    fn restore_realtime_safety(&mut self, tid: i32) {
        self.manager.restore_rt_budget_for_reactor(tid);
    }

    fn request_realtime_round_robin(
        &mut self,
        tid: i32,
        priority: i32,
        reset_on_fork: bool,
    ) -> Result<(), String> {
        if RT_BUDGET_OVERRUN.load(Ordering::Acquire) {
            return Err("RLIMIT_RTTIME overrun disabled realtime scheduling".to_owned());
        }
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
        if unsafe { libc::sched_setscheduler(tid, policy, &parameter) } != 0 {
            return Err(io::Error::last_os_error().to_string());
        }
        if RT_BUDGET_OVERRUN.load(Ordering::Acquire) {
            if set_scheduler_other_preserving_flags(tid).is_err() {
                unsafe { libc::_exit(128 + libc::SIGXCPU) };
            }
            return Err("RLIMIT_RTTIME overrun raced with realtime acquisition".to_owned());
        }
        Ok(())
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

fn claim_reactor_for_sigxcpu(
    active_reactor_tid: &AtomicI32,
    budget_overrun: &AtomicBool,
) -> Option<i32> {
    let tid = active_reactor_tid.load(Ordering::SeqCst);
    if tid <= 0 {
        return None;
    }
    // Publish the sticky failure before revoking the target. A concurrent
    // resume must not re-arm RR after observing that the signal handler has
    // already claimed and is demoting this TID.
    budget_overrun.store(true, Ordering::SeqCst);
    active_reactor_tid
        .compare_exchange(tid, 0, Ordering::SeqCst, Ordering::Relaxed)
        .ok()
}

extern "C" fn sigxcpu_demote(_signal: libc::c_int) {
    let Some(tid) = claim_reactor_for_sigxcpu(&ACTIVE_REACTOR_TID, &RT_BUDGET_OVERRUN) else {
        return;
    };
    let parameter = libc::sched_param { sched_priority: 0 };
    // SAFETY: Linux syscall used directly from the signal path; no allocation,
    // Rust mutex, logging, filesystem, or D-Bus activity occurs here.
    let result = unsafe {
        libc::syscall(
            libc::SYS_sched_setscheduler,
            tid,
            SIGXCPU_DEMOTION_POLICY,
            &parameter,
        )
    };
    if result < 0 {
        unsafe { libc::_exit(128 + libc::SIGXCPU) };
    }
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

fn scheduler_other_policy_for_demotion(current_policy: libc::c_int) -> libc::c_int {
    libc::SCHED_OTHER | (current_policy & libc::SCHED_RESET_ON_FORK)
}

fn set_scheduler_other_preserving_flags(tid: i32) -> Result<(), String> {
    // Linux includes SCHED_RESET_ON_FORK in this return value. Preserve only
    // that known policy flag; do not propagate unknown policy bits.
    let current_policy = unsafe { libc::sched_getscheduler(tid) };
    if current_policy < 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    let policy = scheduler_other_policy_for_demotion(current_policy);
    let parameter = libc::sched_param { sched_priority: 0 };
    if unsafe { libc::sched_setscheduler(tid, policy, &parameter) } != 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    let effective_policy = unsafe { libc::sched_getscheduler(tid) };
    if effective_policy < 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    if effective_policy != policy {
        return Err(format!(
            "scheduler normalization requested policy {policy:#x}, observed {effective_policy:#x}"
        ));
    }
    let mut effective_parameter = libc::sched_param { sched_priority: -1 };
    if unsafe { libc::sched_getparam(tid, &mut effective_parameter) } != 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    if effective_parameter.sched_priority != 0 {
        return Err(format!(
            "scheduler normalization requested priority 0, observed {}",
            effective_parameter.sched_priority
        ));
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

fn required_rttime_warning_headroom(hard_us: u64) -> u64 {
    hard_us
        .saturating_mul(RTTIME_HARD_HEADROOM_NUMERATOR)
        .div_ceil(RTTIME_HARD_HEADROOM_DENOMINATOR)
        .max(RTTIME_MIN_WARNING_HEADROOM_US)
}

fn guarded_rt_limit(
    original: libc::rlimit,
    budget_us: u64,
) -> Result<libc::rlimit, RtTimeLimitSafetyError> {
    let requested = libc::rlim_t::try_from(budget_us)
        .map_err(|_| RtTimeLimitSafetyError::SoftBudgetOutOfRange)?;
    let mut guarded = original;
    let mut soft = original.rlim_cur.min(requested);

    if original.rlim_max != libc::RLIM_INFINITY {
        let hard_wide = u128::from(original.rlim_max);
        let hard_us =
            u64::try_from(hard_wide).map_err(|_| RtTimeLimitSafetyError::SoftBudgetOutOfRange)?;
        let required_headroom_us = required_rttime_warning_headroom(hard_us);
        if hard_us <= required_headroom_us {
            return Err(RtTimeLimitSafetyError::HardLimitTooSmall {
                hard_us,
                required_headroom_us,
            });
        }
        let safe_soft_max = libc::rlim_t::try_from(hard_us - required_headroom_us)
            .map_err(|_| RtTimeLimitSafetyError::SoftBudgetOutOfRange)?;
        soft = soft.min(safe_soft_max);
    }

    if soft == 0 {
        return Err(RtTimeLimitSafetyError::ZeroSoftBudget);
    }
    guarded.rlim_cur = soft;
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
        safety_result: Result<(), &'static str>,
        fair_effective_nice: Option<i32>,
        rr_applies: bool,
        rr_mutations: usize,
        fair_mutations: Vec<SchedulingBackend>,
        normal_mutations: usize,
        safety_restores: usize,
        rt_budget_active: bool,
        active_reactor_tid: i32,
        budget_reactor_tid: i32,
        sigxcpu_handler_installed: bool,
        demotions: u64,
        overrun: bool,
        overrun_after_rr_acquisition: bool,
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
                safety_result: Ok(()),
                fair_effective_nice: None,
                rr_applies: true,
                rr_mutations: 0,
                fair_mutations: Vec::new(),
                normal_mutations: 0,
                safety_restores: 0,
                rt_budget_active: false,
                active_reactor_tid: 0,
                budget_reactor_tid: 0,
                sigxcpu_handler_installed: false,
                demotions: 0,
                overrun: false,
                overrun_after_rr_acquisition: false,
            }
        }

        fn with_fair_success(mut self, backend: SchedulingBackend, nice: i32) -> Self {
            match backend {
                SchedulingBackend::RealtimeKit => self.rtkit_result = Ok(()),
                SchedulingBackend::Direct => self.direct_fair_result = Ok(()),
                _ => unreachable!(),
            }
            self.fair_effective_nice = Some(nice);
            self
        }
    }

    impl SchedulingOperations for FakeOps {
        fn inspect(&mut self, _tid: i32) -> Result<KernelSchedulingState, String> {
            Ok(self.state)
        }

        fn request_rtkit_high_priority_fair(&mut self, _tid: i32, nice: i32) -> Result<(), String> {
            self.fair_mutations.push(SchedulingBackend::RealtimeKit);
            self.rtkit_result.map_err(str::to_owned)?;
            self.state.policy = PolicyKind::Other;
            self.state.priority = 0;
            self.state.nice = self
                .fair_effective_nice
                .unwrap_or_else(|| self.state.nice.min(nice));
            Ok(())
        }

        fn request_direct_high_priority_fair(
            &mut self,
            _tid: i32,
            nice: i32,
        ) -> Result<(), String> {
            self.fair_mutations.push(SchedulingBackend::Direct);
            self.direct_fair_result.map_err(str::to_owned)?;
            self.state.policy = PolicyKind::Other;
            self.state.priority = 0;
            self.state.nice = self
                .fair_effective_nice
                .unwrap_or_else(|| self.state.nice.min(nice));
            Ok(())
        }

        fn request_normal_fair(&mut self, _tid: i32, nice: i32) -> Result<(), String> {
            self.normal_mutations += 1;
            self.state = KernelSchedulingState {
                policy: PolicyKind::Other,
                priority: 0,
                nice,
                reset_on_fork: self.state.reset_on_fork,
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

    impl ReactorSchedulingOperations for FakeOps {
        fn prepare_realtime_safety(&mut self, tid: i32) -> Result<(), String> {
            self.safety_result.map_err(str::to_owned)?;
            if self.rt_budget_active && self.active_reactor_tid != tid {
                return Err("reactor safety owned by another TID".to_owned());
            }
            self.rt_budget_active = true;
            self.active_reactor_tid = tid;
            self.budget_reactor_tid = tid;
            self.sigxcpu_handler_installed = true;
            Ok(())
        }

        fn restore_realtime_safety(&mut self, tid: i32) {
            if self.rt_budget_active && self.budget_reactor_tid == tid {
                self.safety_restores += 1;
                self.rt_budget_active = false;
                self.active_reactor_tid = 0;
                self.budget_reactor_tid = 0;
            }
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
                if self.overrun_after_rr_acquisition {
                    self.overrun = true;
                    self.state = KernelSchedulingState {
                        policy: PolicyKind::Other,
                        priority: 0,
                        nice: self.state.nice,
                        reset_on_fork: self.state.reset_on_fork,
                    };
                    self.demotions += 1;
                    return Err("RLIMIT_RTTIME overrun raced with realtime acquisition".to_owned());
                }
            }
            Ok(())
        }
    }

    #[test]
    fn infinite_hard_limit_caps_soft_budget_at_requested_budget() {
        let original = libc::rlimit {
            rlim_cur: libc::RLIM_INFINITY,
            rlim_max: libc::RLIM_INFINITY,
        };
        let guarded = guarded_rt_limit(original, RT_BUDGET_US).unwrap();

        assert_eq!(guarded.rlim_cur, RT_BUDGET_US as libc::rlim_t);
        assert_eq!(guarded.rlim_max, libc::RLIM_INFINITY);
    }

    #[test]
    fn finite_hard_above_requested_budget_preserves_requested_soft_budget() {
        let original = libc::rlimit {
            rlim_cur: libc::RLIM_INFINITY,
            rlim_max: 200_000,
        };
        let guarded = guarded_rt_limit(original, RT_BUDGET_US).unwrap();

        assert_eq!(guarded.rlim_cur, RT_BUDGET_US as libc::rlim_t);
        assert_eq!(guarded.rlim_max, original.rlim_max);
    }

    #[test]
    fn finite_hard_below_requested_budget_leaves_warning_headroom() {
        let original = libc::rlimit {
            rlim_cur: libc::RLIM_INFINITY,
            rlim_max: 75_000,
        };
        let guarded = guarded_rt_limit(original, RT_BUDGET_US).unwrap();

        assert_eq!(guarded.rlim_cur, 55_000);
        assert_eq!(guarded.rlim_max, original.rlim_max);
    }

    #[test]
    fn smaller_existing_soft_budget_is_preserved_when_safe() {
        let original = libc::rlimit {
            rlim_cur: 25_000,
            rlim_max: 75_000,
        };
        let guarded = guarded_rt_limit(original, RT_BUDGET_US).unwrap();

        assert_eq!(guarded.rlim_cur, original.rlim_cur);
        assert_eq!(guarded.rlim_max, original.rlim_max);
    }

    #[test]
    fn soft_equal_to_finite_hard_is_reduced_below_hard() {
        let original = libc::rlimit {
            rlim_cur: 75_000,
            rlim_max: 75_000,
        };
        let guarded = guarded_rt_limit(original, RT_BUDGET_US).unwrap();

        assert!(guarded.rlim_cur > 0);
        assert!(guarded.rlim_cur < guarded.rlim_max);
        assert_eq!(guarded.rlim_cur, 55_000);
    }

    #[test]
    fn finite_hard_without_warning_interval_disables_direct_realtime() {
        for hard in [
            0,
            RTTIME_MIN_WARNING_HEADROOM_US - 1,
            RTTIME_MIN_WARNING_HEADROOM_US,
        ] {
            let original = libc::rlimit {
                rlim_cur: libc::RLIM_INFINITY,
                rlim_max: hard as libc::rlim_t,
            };
            assert!(guarded_rt_limit(original, RT_BUDGET_US).is_err());
        }
    }

    #[test]
    fn zero_soft_budget_disables_direct_realtime() {
        let original = libc::rlimit {
            rlim_cur: 0,
            rlim_max: libc::RLIM_INFINITY,
        };
        assert!(guarded_rt_limit(original, RT_BUDGET_US).is_err());
    }

    #[test]
    fn every_successful_finite_rt_limit_has_safe_warning_headroom() {
        for hard in [
            1, 19_999, 20_000, 20_001, 40_000, 75_000, 100_000, 200_000, 1_000_000,
        ] {
            let original = libc::rlimit {
                rlim_cur: libc::RLIM_INFINITY,
                rlim_max: hard as libc::rlim_t,
            };
            if let Ok(guarded) = guarded_rt_limit(original, RT_BUDGET_US) {
                let warning_headroom = required_rttime_warning_headroom(hard);
                assert!(guarded.rlim_cur > 0, "hard={hard}");
                assert!(guarded.rlim_cur < guarded.rlim_max, "hard={hard}");
                assert!(
                    u128::from(guarded.rlim_max) - u128::from(guarded.rlim_cur)
                        >= u128::from(warning_headroom),
                    "hard={hard} soft={}",
                    guarded.rlim_cur
                );
                let soft_ticks = u128::from(guarded.rlim_cur).div_ceil(10_000);
                let hard_ticks = u128::from(guarded.rlim_max).div_ceil(10_000);
                assert!(
                    hard_ticks >= soft_ticks + 2,
                    "hard={hard} soft={} soft_ticks={soft_ticks} hard_ticks={hard_ticks}",
                    guarded.rlim_cur
                );
                assert_eq!(guarded.rlim_max, original.rlim_max);
            }
        }
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
    fn worker_fair_transitions_cannot_disable_reactor_realtime_safety() {
        let mut reactor = FakeOps::normal(0);
        let reactor_snapshot = configure_reactor_policy_with_rtkit(
            SchedulingQosPolicy::Auto,
            700,
            0,
            true,
            &mut reactor,
        )
        .expect("reactor RR safety is active");
        assert_eq!(
            reactor_snapshot.effective_class,
            EffectiveSchedulingClass::RealtimeRoundRobin { priority: 1 }
        );
        assert!(reactor.rt_budget_active);
        assert_eq!(reactor.active_reactor_tid, 700);
        assert_eq!(reactor.budget_reactor_tid, 700);
        assert!(reactor.sigxcpu_handler_installed);

        for role in [
            SchedulingThreadRole::KmsCommit,
            SchedulingThreadRole::Render,
            SchedulingThreadRole::Background,
        ] {
            let mut worker = FakeOps::normal(0);
            worker.direct_fair_result = Ok(());
            worker.rt_budget_active = reactor.rt_budget_active;
            worker.active_reactor_tid = reactor.active_reactor_tid;
            worker.budget_reactor_tid = reactor.budget_reactor_tid;
            worker.sigxcpu_handler_installed = reactor.sigxcpu_handler_installed;
            if role == SchedulingThreadRole::Background {
                worker.state.nice = -3;
            }

            let snapshot =
                configure_worker_policy(role, SchedulingQosPolicy::Auto, 701, 0, &mut worker);

            assert_eq!(snapshot.role, role);
            assert!(worker.rt_budget_active, "role={role:?}");
            assert_eq!(worker.active_reactor_tid, 700, "role={role:?}");
            assert_eq!(worker.budget_reactor_tid, 700, "role={role:?}");
            assert!(worker.sigxcpu_handler_installed, "role={role:?}");
            assert_eq!(worker.safety_restores, 0, "role={role:?}");
        }
    }

    #[test]
    fn reactor_rr_to_fair_fallback_removes_only_realtime_budget_state() {
        let mut ops = FakeOps::normal(0).with_fair_success(SchedulingBackend::Direct, -8);
        ops.rr_result = Err("EPERM");
        let snapshot =
            configure_reactor_policy_with_rtkit(SchedulingQosPolicy::Auto, 702, 0, false, &mut ops)
                .expect("Auto falls back to FAIR");

        assert_eq!(snapshot.backend, SchedulingBackend::Direct);
        assert_eq!(snapshot.effective_nice, -8);
        assert!(!ops.rt_budget_active);
        assert_eq!(ops.active_reactor_tid, 0);
        assert!(ops.sigxcpu_handler_installed);
    }

    #[test]
    fn rtkit_and_direct_fair_protection_survive_session_suspend_and_resume() {
        for (backend, allow_rtkit) in [
            (SchedulingBackend::RealtimeKit, true),
            (SchedulingBackend::Direct, false),
        ] {
            let mut ops = FakeOps::normal(0).with_fair_success(backend, -7);
            ops.rr_result = Err("EPERM");
            let startup = configure_reactor_policy_with_rtkit(
                SchedulingQosPolicy::Auto,
                703,
                0,
                allow_rtkit,
                &mut ops,
            )
            .expect("FAIR fallback established at startup");
            let startup_fair_calls = ops.fair_mutations.len();

            prepare_reactor_for_session_suspend(703, 0, &mut ops)
                .expect("FAIR is safe across a suspend boundary");
            assert_eq!(ops.state.policy, PolicyKind::Other);
            assert_eq!(ops.state.nice, -7);
            assert_eq!(ops.fair_mutations.len(), startup_fair_calls);

            let resumed = reacquire_reactor_policy_after_suspend(
                SchedulingQosPolicy::Auto,
                703,
                0,
                false,
                Some(&startup),
                &mut ops,
            )
            .expect("resume retains already-effective FAIR protection");
            assert_eq!(resumed.backend, backend);
            assert_eq!(
                resumed.effective_class,
                EffectiveSchedulingClass::Fair { nice: -7 }
            );
            assert_eq!(ops.fair_mutations.len(), startup_fair_calls);
            assert_eq!(ops.rr_mutations, 1);
        }
    }

    #[test]
    fn direct_rr_is_demoted_for_suspend_and_reacquired_after_recovery() {
        let mut ops = FakeOps::normal(0);
        let startup =
            configure_reactor_policy_with_rtkit(SchedulingQosPolicy::Auto, 704, 0, true, &mut ops)
                .expect("RR acquired");
        assert!(ops.rt_budget_active);

        prepare_reactor_for_session_suspend(704, 0, &mut ops)
            .expect("RR is removed before session suspension");
        assert_eq!(ops.state.policy, PolicyKind::Other);
        assert_eq!(ops.state.nice, 0);
        assert!(!ops.rt_budget_active);
        assert_eq!(ops.active_reactor_tid, 0);

        let resumed = reacquire_reactor_policy_after_suspend(
            SchedulingQosPolicy::Auto,
            704,
            0,
            false,
            Some(&startup),
            &mut ops,
        )
        .expect("direct RR is reacquired after recovery");
        assert_eq!(
            resumed.effective_class,
            EffectiveSchedulingClass::RealtimeRoundRobin { priority: 1 }
        );
        assert!(ops.rt_budget_active);
        assert_eq!(ops.active_reactor_tid, 704);
        assert_eq!(ops.rr_mutations, 2);
    }

    #[test]
    fn rt_budget_overrun_suppresses_realtime_reacquisition_after_suspend() {
        let mut ops = FakeOps::normal(0);
        let startup =
            configure_reactor_policy_with_rtkit(SchedulingQosPolicy::Auto, 705, 0, true, &mut ops)
                .expect("RR acquired");
        ops.overrun = true;
        ops.state = KernelSchedulingState {
            policy: PolicyKind::Other,
            priority: 0,
            nice: 0,
            reset_on_fork: false,
        };
        ops.active_reactor_tid = 0;

        prepare_reactor_for_session_suspend(705, 0, &mut ops)
            .expect("suspend retires the already-demoted RT budget");
        let rr_mutations = ops.rr_mutations;
        let resumed = reacquire_reactor_policy_after_suspend(
            SchedulingQosPolicy::Auto,
            705,
            0,
            true,
            Some(&startup),
            &mut ops,
        )
        .expect("overrun recovery remains fail-open without RR");

        assert_eq!(
            resumed.effective_class,
            EffectiveSchedulingClass::Fair { nice: 0 }
        );
        assert!(resumed.rt_budget_overrun);
        assert!(!ops.rt_budget_active);
        assert_eq!(ops.rr_mutations, rr_mutations);
    }

    #[test]
    fn shutdown_normalizes_elevated_fair_policy_and_removes_rt_budget() {
        for backend in [SchedulingBackend::RealtimeKit, SchedulingBackend::Direct] {
            let mut ops = FakeOps::normal(0).with_fair_success(backend, -7);
            ops.rr_result = Err("EPERM");
            let _ = configure_reactor_policy_with_rtkit(
                SchedulingQosPolicy::Auto,
                706,
                0,
                backend == SchedulingBackend::RealtimeKit,
                &mut ops,
            )
            .expect("FAIR fallback established");

            prepare_reactor_for_shutdown(706, 0, &mut ops)
                .expect("shutdown normalizes the reactor");
            assert_eq!(ops.state.policy, PolicyKind::Other);
            assert_eq!(ops.state.nice, 0);
            assert!(!ops.rt_budget_active);
            assert_eq!(ops.active_reactor_tid, 0);
        }
    }

    #[test]
    fn signal_cleanup_keeps_failsafe_disposition_for_pending_group_signal() {
        let source = include_str!("scheduling_qos.rs");
        let cleanup = source
            .split_once("fn restore_rt_budget_for_reactor(&self, tid: i32)")
            .expect("RT budget cleanup exists")
            .1
            .split_once("\n    }\n}")
            .expect("RT budget cleanup body ends before manager impl")
            .0;
        assert!(cleanup.contains("pthread_sigmask"));
        assert!(cleanup.contains("setrlimit(libc::RLIMIT_RTTIME"));
        let active_clear = cleanup
            .find("ACTIVE_REACTOR_TID.compare_exchange")
            .expect("reactor authority is revoked before the prior mask is restored");
        let limit_restore = cleanup
            .find("setrlimit(libc::RLIMIT_RTTIME")
            .expect("process RT limit is restored");
        let mask_restore = cleanup
            .find("pthread_sigmask(libc::SIG_SETMASK")
            .expect("reactor signal mask is restored last");
        assert!(active_clear < limit_restore && limit_restore < mask_restore);
        assert!(
            !cleanup.contains("sigaction("),
            "SIGXCPU disposition must remain the fail-safe while a group signal may be pending"
        );
        assert!(source.contains("SIGXCPU_HANDLER_INSTALLED.store(true"));
        assert!(source.contains("fn install_sigxcpu_handler()"));
        let handler = source
            .split_once("extern \"C\" fn sigxcpu_demote")
            .expect("process-directed SIGXCPU handler exists")
            .1
            .split_once("\nfn inspect_kernel_state")
            .expect("signal handler ends before scheduler inspection")
            .0;
        assert!(handler.contains("claim_reactor_for_sigxcpu"));
        assert!(!handler.contains("ACTIVE_REACTOR_TID.store"));
        let claim = source
            .split_once("fn claim_reactor_for_sigxcpu(")
            .expect("signal claim helper exists")
            .1
            .split_once("extern \"C\" fn sigxcpu_demote")
            .expect("claim helper precedes handler")
            .0;
        let overrun = claim
            .find("budget_overrun.store(true")
            .expect("overrun becomes sticky before the target is cleared");
        let target_clear = claim
            .find(".compare_exchange")
            .expect("claim atomically clears the reactor target");
        assert!(overrun < target_clear);

        let promotion = source
            .split_once("pub fn promote_current_reactor()")
            .expect("reactor promotion exists")
            .1
            .split_once("pub fn reacquire_current_reactor_after_session_resume()")
            .expect("resume entry follows promotion")
            .0;
        let resume = source
            .split_once("pub fn reacquire_current_reactor_after_session_resume()")
            .expect("resume entry exists")
            .1
            .split_once("pub fn prepare_current_reactor_for_session_suspend")
            .expect("suspend entry follows resume")
            .0;
        assert!(!promotion.contains("ACTIVE_REACTOR_TID.store"));
        assert!(!resume.contains("ACTIVE_REACTOR_TID.store"));

        let process = include_str!("../process.rs");
        let signalfd = process
            .split_once("fn create_runtime_signalfd()")
            .expect("runtime signalfd setup exists")
            .1;
        assert!(!signalfd.contains("SIGXCPU"));
        assert!(signalfd.contains("SIGCHLD"));
        assert!(signalfd.contains("SIGTERM"));
    }

    #[test]
    fn sigxcpu_claim_makes_overrun_sticky_before_clearing_the_reactor_tid() {
        let active_reactor_tid = AtomicI32::new(712);
        let budget_overrun = AtomicBool::new(false);

        assert_eq!(
            claim_reactor_for_sigxcpu(&active_reactor_tid, &budget_overrun),
            Some(712)
        );
        assert!(budget_overrun.load(Ordering::Acquire));
        assert_eq!(active_reactor_tid.load(Ordering::Acquire), 0);
    }

    #[test]
    fn sigxcpu_without_an_active_reactor_does_not_mark_a_budget_overrun() {
        let active_reactor_tid = AtomicI32::new(0);
        let budget_overrun = AtomicBool::new(false);

        assert_eq!(
            claim_reactor_for_sigxcpu(&active_reactor_tid, &budget_overrun),
            None
        );
        assert!(!budget_overrun.load(Ordering::Acquire));
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
            EffectiveSchedulingClass::Fair { nice: -10 }
        );
        assert!(snapshot.degradation_reason.is_some());
        assert_eq!(ops.safety_restores, 1);
    }

    #[test]
    fn unsafe_finite_hard_limit_disables_direct_rr_and_auto_uses_fair() {
        let mut ops = FakeOps::normal(0).with_fair_success(SchedulingBackend::Direct, -8);
        ops.safety_result = Err("hard RTTIME limit lacks warning headroom");
        let snapshot = configure_reactor_policy(SchedulingQosPolicy::Auto, 431, 0, &mut ops)
            .expect("unsafe direct RR budget degrades to FAIR in Auto");

        assert_eq!(ops.rr_mutations, 0);
        assert!(!ops.rt_budget_active);
        assert_eq!(snapshot.backend, SchedulingBackend::Direct);
        assert_eq!(
            snapshot.effective_class,
            EffectiveSchedulingClass::Fair { nice: -8 }
        );
        assert!(
            snapshot
                .degradation_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("warning headroom"))
        );
    }

    #[test]
    fn unsafe_finite_hard_limit_rejects_require_without_requesting_rr() {
        let mut ops = FakeOps::normal(0);
        ops.safety_result = Err("hard RTTIME limit lacks warning headroom");
        let error = configure_reactor_policy(SchedulingQosPolicy::Require, 432, 0, &mut ops)
            .expect_err("Require rejects a reactor without a safe RT budget");

        assert_eq!(ops.rr_mutations, 0);
        assert!(!ops.rt_budget_active);
        assert!(
            error
                .snapshot
                .degradation_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("warning headroom"))
        );
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
    fn scheduler_other_request_preserves_only_reset_on_fork() {
        let unknown_flag = 0x2000_0000;
        assert_eq!(
            scheduler_other_policy_for_demotion(
                libc::SCHED_RR | libc::SCHED_RESET_ON_FORK | unknown_flag,
            ),
            libc::SCHED_OTHER | libc::SCHED_RESET_ON_FORK
        );
        assert_eq!(
            scheduler_other_policy_for_demotion(libc::SCHED_OTHER),
            libc::SCHED_OTHER
        );
    }

    #[test]
    fn current_thread_can_be_normalized_without_clearing_or_inventing_flags() {
        let tid = unsafe { libc::gettid() };
        let current = unsafe { libc::sched_getscheduler(tid) };
        assert!(current >= 0);
        let expected = scheduler_other_policy_for_demotion(current);

        set_scheduler_other_preserving_flags(tid).unwrap();

        assert_eq!(unsafe { libc::sched_getscheduler(tid) }, expected);
        let mut parameter = libc::sched_param { sched_priority: -1 };
        assert_eq!(unsafe { libc::sched_getparam(tid, &mut parameter) }, 0);
        assert_eq!(parameter.sched_priority, 0);
    }

    fn modeled_unprivileged_scheduler_change(
        current_policy: libc::c_int,
        requested_policy: libc::c_int,
        priority: i32,
        cap_sys_nice: bool,
        rlimit_rtprio: i32,
    ) -> Result<libc::c_int, i32> {
        let current_reset = current_policy & libc::SCHED_RESET_ON_FORK != 0;
        let requested_reset = requested_policy & libc::SCHED_RESET_ON_FORK != 0;
        if current_reset && !requested_reset && !cap_sys_nice {
            return Err(libc::EPERM);
        }
        let requested_class = requested_policy & !libc::SCHED_RESET_ON_FORK;
        if requested_class == libc::SCHED_RR && !cap_sys_nice && priority > rlimit_rtprio {
            return Err(libc::EPERM);
        }
        Ok(requested_policy)
    }

    #[test]
    fn rlimit_rtprio_only_can_acquire_and_demote_rr_without_clearing_reset() {
        let mut policy = libc::SCHED_OTHER;
        let cap_sys_nice = false;
        let rlimit_rtprio = 1;
        policy = modeled_unprivileged_scheduler_change(
            policy,
            libc::SCHED_RR | libc::SCHED_RESET_ON_FORK,
            REACTOR_RR_PRIORITY,
            cap_sys_nice,
            rlimit_rtprio,
        )
        .expect("RLIMIT_RTPRIO permits RR priority 1 without CAP_SYS_NICE");

        policy = modeled_unprivileged_scheduler_change(
            policy,
            scheduler_other_policy_for_demotion(policy),
            0,
            cap_sys_nice,
            rlimit_rtprio,
        )
        .expect("normal demotion retains reset-on-fork");
        assert_eq!(policy, libc::SCHED_OTHER | libc::SCHED_RESET_ON_FORK);
        assert_eq!(
            modeled_unprivileged_scheduler_change(
                policy,
                libc::SCHED_OTHER,
                0,
                cap_sys_nice,
                rlimit_rtprio,
            ),
            Err(libc::EPERM),
            "bare SCHED_OTHER would illegally clear the established flag"
        );
    }

    #[test]
    fn overrun_during_rr_acquisition_uses_flag_preserving_demotion() {
        let mut ops = FakeOps::normal(0);
        ops.overrun_after_rr_acquisition = true;
        let snapshot =
            configure_reactor_policy_with_rtkit(SchedulingQosPolicy::Auto, 711, 0, false, &mut ops)
                .expect("the overrun race degrades without terminating the process");

        assert!(ops.overrun);
        assert_eq!(ops.demotions, 1);
        assert_eq!(ops.state.policy, PolicyKind::Other);
        assert!(ops.state.reset_on_fork);
        assert!(snapshot.rt_budget_overrun);
        assert_eq!(snapshot.demotion_count, 1);
        assert_eq!(
            snapshot.effective_class,
            EffectiveSchedulingClass::Fair { nice: 0 }
        );
        assert!(snapshot.reset_on_fork_effective);
        let source = include_str!("scheduling_qos.rs");
        let reactor_ops = source
            .split_once("impl ReactorSchedulingOperations for LinuxSchedulingOperations")
            .expect("Linux reactor operations are isolated")
            .1
            .split_once("\nimpl QosManager")
            .expect("reactor scheduler implementation ends before the manager")
            .0;
        assert!(reactor_ops.contains("set_scheduler_other_preserving_flags(tid).is_err()"));
    }

    #[test]
    fn reset_on_fork_remains_valid_through_suspend_and_resume() {
        let mut ops = FakeOps::normal(0);
        ops.state = KernelSchedulingState {
            policy: PolicyKind::RoundRobin,
            priority: REACTOR_RR_PRIORITY,
            nice: 0,
            reset_on_fork: true,
        };
        ops.rt_budget_active = true;
        ops.active_reactor_tid = 712;
        ops.budget_reactor_tid = 712;

        prepare_reactor_for_session_suspend(712, 0, &mut ops)
            .expect("suspend removes RR while retaining reset-on-fork");
        assert_eq!(ops.state.policy, PolicyKind::Other);
        assert!(ops.state.reset_on_fork);

        let resumed = reacquire_reactor_policy_after_suspend(
            SchedulingQosPolicy::Auto,
            712,
            0,
            false,
            None,
            &mut ops,
        )
        .expect("resume reacquires verified RR");
        assert_eq!(ops.state.policy, PolicyKind::RoundRobin);
        assert!(ops.state.reset_on_fork);
        assert!(resumed.reset_on_fork_effective);
    }

    #[test]
    fn session_resume_fair_fallback_retains_reset_on_fork() {
        let mut ops = FakeOps::normal(0);
        ops.state = KernelSchedulingState {
            policy: PolicyKind::RoundRobin,
            priority: REACTOR_RR_PRIORITY,
            nice: 0,
            reset_on_fork: true,
        };
        ops.rt_budget_active = true;
        ops.active_reactor_tid = 715;
        ops.budget_reactor_tid = 715;
        prepare_reactor_for_session_suspend(715, 0, &mut ops)
            .expect("suspend retains the inherited reset-on-fork property");
        ops.rr_result = Err("EPERM");
        ops.direct_fair_result = Ok(());
        ops.fair_effective_nice = Some(-8);

        let resumed = reacquire_reactor_policy_after_suspend(
            SchedulingQosPolicy::Auto,
            715,
            0,
            false,
            None,
            &mut ops,
        )
        .expect("resume falls back to FAIR without clearing the flag");

        assert_eq!(ops.state.policy, PolicyKind::Other);
        assert!(ops.state.reset_on_fork);
        assert_eq!(
            resumed.effective_class,
            EffectiveSchedulingClass::Fair { nice: -8 }
        );
        assert!(resumed.reset_on_fork_effective);
    }

    #[test]
    fn fair_fallback_keeps_reset_on_fork_and_does_not_invent_it_for_workers() {
        let mut reactor = FakeOps::normal(0);
        reactor.state = KernelSchedulingState {
            policy: PolicyKind::RoundRobin,
            priority: REACTOR_RR_PRIORITY,
            nice: 0,
            reset_on_fork: true,
        };
        reactor.safety_result = Err("RT budget unavailable");
        reactor.direct_fair_result = Ok(());
        reactor.fair_effective_nice = Some(-8);
        let fallback = configure_reactor_policy_with_rtkit(
            SchedulingQosPolicy::Auto,
            713,
            0,
            false,
            &mut reactor,
        )
        .expect("failed RR safety falls back to FAIR");
        assert_eq!(
            fallback.effective_class,
            EffectiveSchedulingClass::Fair { nice: -8 }
        );
        assert!(fallback.reset_on_fork_effective);

        let mut worker = FakeOps::normal(0);
        worker.direct_fair_result = Ok(());
        let worker_snapshot = configure_worker_policy(
            SchedulingThreadRole::Background,
            SchedulingQosPolicy::Auto,
            714,
            0,
            &mut worker,
        );
        assert!(!worker_snapshot.reset_on_fork_effective);
        assert!(!worker.state.reset_on_fork);
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
                reset_on_fork: true,
            }
        );
        assert_eq!(
            scheduler_other_policy_for_demotion(libc::SCHED_RR | libc::SCHED_RESET_ON_FORK),
            libc::SCHED_OTHER | libc::SCHED_RESET_ON_FORK
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
            .find("set_scheduler_other_preserving_flags(tid)")
            .expect("creator policy is ordinary before spawn");
        let normal_nice = function
            .find("set_nice(tid, desired.nice)")
            .expect("creator nice is ordinary before spawn");
        let spawn = function
            .find("let spawned = spawn()")
            .expect("classified child is spawned");
        let restore = function[spawn..]
            .find("restore_thread_scheduler(tid, prior)")
            .map(|offset| spawn + offset)
            .expect("creator scheduler is restored after helper creation");
        assert!(normalize < spawn);
        assert!(normal_policy < spawn);
        assert!(normal_nice < spawn);
        assert!(spawn < restore);

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

        let factory = source
            .split_once("pub fn spawn_role_thread")
            .expect("classified thread factory exists")
            .1;
        let reject_reactor = factory
            .find("role == SchedulingThreadRole::NativeReactor")
            .expect("helper factory rejects reactor role");
        let spawn = factory
            .find("let spawn = ||")
            .expect("helper spawn closure exists");
        assert!(reject_reactor < spawn);
        assert!(source.contains("briefly removes its RR starvation protection"));
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
        assert!(body.contains("claim_reactor_for_sigxcpu"));
        assert!(body.contains("SIGXCPU_DEMOTION_POLICY"));
        assert!(body.contains("sched_priority: 0"));
        assert_eq!(
            SIGXCPU_DEMOTION_POLICY,
            libc::SCHED_OTHER | libc::SCHED_RESET_ON_FORK
        );
        assert!(source.contains("budget_overrun.store(true, Ordering::SeqCst)"));
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
        let qos = include_str!("scheduling_qos.rs");
        let promotion = qos
            .split_once("pub fn promote_current_reactor()")
            .expect("QoS promotion entry exists")
            .1;
        let policy_guard = promotion
            .find("manager.policy != SchedulingQosPolicy::Off")
            .expect("same-binary off mode skips helper priming");
        let trace_prime = promotion
            .find("prime_enabled_trace_sinks()")
            .expect("optional lazy trace workers are primed before promotion");
        let configuration = promotion
            .find("configure_reactor_policy(")
            .expect("reactor promotion follows helper setup");
        assert!(policy_guard < trace_prime && trace_prime < configuration);
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
        assert!(qos.contains("pthread_sigmask(libc::SIG_UNBLOCK"));
        assert!(qos.contains("libc::SIG_SETMASK"));

        let worker_impl = qos
            .split_once("impl SchedulingOperations for LinuxSchedulingOperations")
            .expect("worker scheduling operations exist")
            .1
            .split_once("impl ReactorSchedulingOperations for LinuxSchedulingOperations")
            .expect("reactor safety operations are a separate implementation")
            .0;
        assert!(!worker_impl.contains("restore_rt_budget"));
        assert!(!worker_impl.contains("prepare_rt_budget"));
        assert!(!worker_impl.contains("ACTIVE_REACTOR_TID"));

        let resume = qos
            .split_once("pub fn reacquire_current_reactor_after_session_resume()")
            .expect("session resume QoS boundary exists")
            .1
            .split_once("pub fn prepare_current_reactor_for_session_suspend")
            .expect("session suspend QoS boundary follows resume")
            .0;
        assert!(!resume.contains("zbus::"));
        let suspend = cycle
            .split_once("fn suspend_native_session")
            .expect("session suspend path exists")
            .1;
        assert!(suspend.contains("prepare_current_reactor_for_session_suspend"));
        let shutdown = include_str!("../native_output/runtime/shutdown_cycle.rs");
        assert!(shutdown.contains("demote_current_reactor()"));

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
