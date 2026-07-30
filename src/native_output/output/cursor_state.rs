#![allow(dead_code)]

use crate::native_output::{OutputTransactionId, presentation::plane::CursorRevision};
use oblivion_one::native::kms::{AtomicCursorVisualState, PageFlipToken};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AtomicCursorDirty {
    pub(crate) position: bool,
    pub(crate) visibility: bool,
    pub(crate) image: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AtomicCursorCounters {
    pub(crate) image_uploads: u64,
    pub(crate) client_image_uploads: u64,
    pub(crate) image_cache_hits: u64,
    pub(crate) position_submissions: u64,
    pub(crate) primary_submissions: u64,
    pub(crate) updates_requested: u64,
    pub(crate) updates_submitted: u64,
    pub(crate) updates_completed: u64,
    pub(crate) updates_coalesced: u64,
    pub(crate) hidden_updates_suppressed: u64,
    pub(crate) test_failures: u64,
    pub(crate) submit_failures: u64,
    pub(crate) software_fallbacks: u64,
    pub(crate) composed_cursor_fallbacks: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkerQueuedCursorSubmission {
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) token: PageFlipToken,
    pub(crate) cursor_epoch: u64,
    pub(crate) revision: CursorRevision,
    pub(crate) visual_state: AtomicCursorVisualState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorRevisionTracker {
    desired: CursorRevision,
    submitted: CursorRevision,
    presented: CursorRevision,
}

impl CursorRevisionTracker {
    pub(crate) const fn new() -> Self {
        let initial = CursorRevision::initial();
        Self {
            desired: initial,
            submitted: initial,
            presented: initial,
        }
    }

    pub(crate) const fn desired(self) -> CursorRevision {
        self.desired
    }

    pub(crate) const fn presented(self) -> CursorRevision {
        self.presented
    }

    pub(crate) fn advance_image(&mut self) {
        self.desired = self.desired.advance_image();
    }

    pub(crate) fn advance_motion(&mut self) {
        self.desired = self.desired.advance_motion();
    }

    pub(crate) fn advance_visibility(&mut self) {
        self.desired = self.desired.advance_visibility();
    }

    pub(crate) fn mark_submitted(&mut self, revision: CursorRevision) {
        self.submitted = revision;
    }

    pub(crate) fn mark_presented(&mut self) {
        self.presented = self.submitted;
    }

    pub(crate) fn mark_initial_presented(&mut self) {
        self.submitted = self.desired;
        self.presented = self.desired;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorQuarantineReason {
    UnsupportedSize,
    UnsupportedFormat,
    UnsupportedModifier,
    UnsupportedTransform,
    UnsupportedHotspot,
    TestOnlyRejected,
    PermanentSubmitRejection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorCapabilityStatus {
    Unknown,
    Proven,
    Quarantined {
        reason: CursorQuarantineReason,
        failure_count: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorPlaneLifecycle {
    generation: u64,
    initial_clear_confirmed: bool,
    capability_status: CursorCapabilityStatus,
}

impl CursorPlaneLifecycle {
    pub(crate) const fn new(generation: u64) -> Self {
        Self {
            generation,
            initial_clear_confirmed: false,
            capability_status: CursorCapabilityStatus::Unknown,
        }
    }

    pub(crate) const fn generation(self) -> u64 {
        self.generation
    }

    pub(crate) const fn initial_clear_required(self) -> bool {
        !self.initial_clear_confirmed
    }

    pub(crate) const fn capability_status(self) -> CursorCapabilityStatus {
        self.capability_status
    }

    pub(crate) fn mark_proven(&mut self) {
        self.capability_status = CursorCapabilityStatus::Proven;
    }

    pub(crate) fn invalidate_capability(&mut self) {
        self.capability_status = CursorCapabilityStatus::Unknown;
    }

    pub(crate) fn quarantine(&mut self, reason: CursorQuarantineReason) {
        let failure_count = match self.capability_status {
            CursorCapabilityStatus::Quarantined {
                reason: existing,
                failure_count,
            } if existing == reason => failure_count.saturating_add(1),
            CursorCapabilityStatus::Unknown
            | CursorCapabilityStatus::Proven
            | CursorCapabilityStatus::Quarantined { .. } => 1,
        };
        self.capability_status = CursorCapabilityStatus::Quarantined {
            reason,
            failure_count,
        };
    }

    pub(crate) fn confirm_initial_clear(&mut self, generation: u64) -> bool {
        if generation != self.generation || self.initial_clear_confirmed {
            return false;
        }
        self.initial_clear_confirmed = true;
        true
    }

    pub(crate) fn rearm_generation(&mut self, generation: u64) -> bool {
        if generation == self.generation {
            return false;
        }
        self.generation = generation;
        self.initial_clear_confirmed = false;
        self.capability_status = CursorCapabilityStatus::Unknown;
        true
    }
}
