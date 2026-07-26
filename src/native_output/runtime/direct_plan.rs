#![allow(dead_code)]

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectScanoutRuntimeBlocker {
    PolicyOff,
    WorkerUnavailable,
    WorkerQueueFull,
    SessionInactive,
    ShutdownActive,
    OutputTransition,
    PrimaryCommitPending,
    SoftwareCursorVisible,
    CursorAssignmentUnsupported,
    AcquireNotReady,
    BufferDeviceUnproven,
    SameContent,
}

impl DirectScanoutRuntimeBlocker {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::PolicyOff => "policy_off",
            Self::WorkerUnavailable => "worker_unavailable",
            Self::WorkerQueueFull => "worker_queue_full",
            Self::SessionInactive => "session_inactive",
            Self::ShutdownActive => "shutdown_active",
            Self::OutputTransition => "output_transition",
            Self::PrimaryCommitPending => "primary_commit_pending",
            Self::SoftwareCursorVisible => "software_cursor_visible",
            Self::CursorAssignmentUnsupported => "cursor_assignment_unsupported",
            Self::AcquireNotReady => "acquire_not_ready",
            Self::BufferDeviceUnproven => "buffer_device_unproven",
            Self::SameContent => "same_content",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectScanoutPlanInput {
    pub(crate) policy_enabled: bool,
    pub(crate) worker_running: bool,
    pub(crate) worker_admission_available: bool,
    pub(crate) session_active: bool,
    pub(crate) shutdown_active: bool,
    pub(crate) output_transition: bool,
    pub(crate) primary_commit_pending: bool,
    pub(crate) software_cursor_visible: bool,
    pub(crate) cursor_assignment_supported: bool,
    pub(crate) acquire_ready: bool,
    pub(crate) buffer_device_proven: bool,
    pub(crate) candidate_key: DirectScanoutCandidateKey,
    pub(crate) presented_key: Option<DirectScanoutCandidateKey>,
    pub(crate) pending_key: Option<DirectScanoutCandidateKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectScanoutDecision {
    Eligible,
    Blocked(DirectScanoutRuntimeBlocker),
}

pub(crate) fn plan_direct_scanout(input: DirectScanoutPlanInput) -> DirectScanoutDecision {
    let blocker = if !input.policy_enabled {
        Some(DirectScanoutRuntimeBlocker::PolicyOff)
    } else if !input.worker_running {
        Some(DirectScanoutRuntimeBlocker::WorkerUnavailable)
    } else if !input.worker_admission_available {
        Some(DirectScanoutRuntimeBlocker::WorkerQueueFull)
    } else if !input.session_active {
        Some(DirectScanoutRuntimeBlocker::SessionInactive)
    } else if input.shutdown_active {
        Some(DirectScanoutRuntimeBlocker::ShutdownActive)
    } else if input.output_transition {
        Some(DirectScanoutRuntimeBlocker::OutputTransition)
    } else if input.primary_commit_pending {
        Some(DirectScanoutRuntimeBlocker::PrimaryCommitPending)
    } else if input.software_cursor_visible {
        Some(DirectScanoutRuntimeBlocker::SoftwareCursorVisible)
    } else if !input.cursor_assignment_supported {
        Some(DirectScanoutRuntimeBlocker::CursorAssignmentUnsupported)
    } else if !input.acquire_ready {
        Some(DirectScanoutRuntimeBlocker::AcquireNotReady)
    } else if !input.buffer_device_proven {
        Some(DirectScanoutRuntimeBlocker::BufferDeviceUnproven)
    } else if input.presented_key == Some(input.candidate_key)
        || input.pending_key == Some(input.candidate_key)
    {
        Some(DirectScanoutRuntimeBlocker::SameContent)
    } else {
        None
    };

    blocker.map_or(
        DirectScanoutDecision::Eligible,
        DirectScanoutDecision::Blocked,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> DirectScanoutCandidateKey {
        let content = OutputContentKey::new(
            7,
            std::num::NonZeroU64::new(42).expect("buffer id"),
            ContentEpochId::new(std::num::NonZeroU64::new(3).expect("content epoch")),
            1920,
            1080,
            0x3432_5241,
            0,
            0,
            1_000,
            0,
        );
        DirectScanoutCandidateKey {
            content,
            output_generation: 1,
            cursor_plan_key: None,
            color_epoch: 0,
        }
    }

    #[test]
    fn policy_blocker_precedes_scene_work() {
        let key = test_key();
        assert_eq!(
            plan_direct_scanout(DirectScanoutPlanInput {
                policy_enabled: false,
                worker_running: true,
                worker_admission_available: true,
                session_active: true,
                shutdown_active: false,
                output_transition: false,
                primary_commit_pending: false,
                software_cursor_visible: false,
                cursor_assignment_supported: true,
                acquire_ready: true,
                buffer_device_proven: true,
                candidate_key: key,
                presented_key: None,
                pending_key: None,
            }),
            DirectScanoutDecision::Blocked(DirectScanoutRuntimeBlocker::PolicyOff)
        );
    }

    #[test]
    fn worker_unavailable_precedes_candidate_import() {
        let key = test_key();
        assert_eq!(
            plan_direct_scanout(DirectScanoutPlanInput {
                policy_enabled: true,
                worker_running: false,
                worker_admission_available: true,
                session_active: true,
                shutdown_active: false,
                output_transition: false,
                primary_commit_pending: false,
                software_cursor_visible: false,
                cursor_assignment_supported: true,
                acquire_ready: true,
                buffer_device_proven: true,
                candidate_key: key,
                presented_key: None,
                pending_key: None,
            }),
            DirectScanoutDecision::Blocked(DirectScanoutRuntimeBlocker::WorkerUnavailable)
        );
    }

    #[test]
    fn shutdown_precedes_primary_admission() {
        let key = test_key();
        assert_eq!(
            plan_direct_scanout(DirectScanoutPlanInput {
                policy_enabled: true,
                worker_running: true,
                worker_admission_available: true,
                session_active: true,
                shutdown_active: true,
                output_transition: false,
                primary_commit_pending: false,
                software_cursor_visible: false,
                cursor_assignment_supported: true,
                acquire_ready: true,
                buffer_device_proven: true,
                candidate_key: key,
                presented_key: None,
                pending_key: None,
            }),
            DirectScanoutDecision::Blocked(DirectScanoutRuntimeBlocker::ShutdownActive)
        );
    }

    #[test]
    fn visible_software_cursor_blocks_direct_scanout() {
        let key = test_key();
        assert_eq!(
            plan_direct_scanout(DirectScanoutPlanInput {
                policy_enabled: true,
                worker_running: true,
                worker_admission_available: true,
                session_active: true,
                shutdown_active: false,
                output_transition: false,
                primary_commit_pending: false,
                software_cursor_visible: true,
                cursor_assignment_supported: true,
                acquire_ready: true,
                buffer_device_proven: true,
                candidate_key: key,
                presented_key: None,
                pending_key: None,
            }),
            DirectScanoutDecision::Blocked(DirectScanoutRuntimeBlocker::SoftwareCursorVisible)
        );
    }

    #[test]
    fn identical_presented_content_returns_same_content() {
        let key = test_key();
        assert_eq!(
            plan_direct_scanout(DirectScanoutPlanInput {
                policy_enabled: true,
                worker_running: true,
                worker_admission_available: true,
                session_active: true,
                shutdown_active: false,
                output_transition: false,
                primary_commit_pending: false,
                software_cursor_visible: false,
                cursor_assignment_supported: true,
                acquire_ready: true,
                buffer_device_proven: true,
                candidate_key: key,
                presented_key: Some(key),
                pending_key: None,
            }),
            DirectScanoutDecision::Blocked(DirectScanoutRuntimeBlocker::SameContent)
        );
    }
}
