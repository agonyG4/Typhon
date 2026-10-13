#![allow(dead_code)]

pub(crate) use oblivion_one::native::adaptive_buffering::{
    TripleCapability, TripleCapabilityBlocker,
};
use oblivion_one::native::kms::PageFlipToken;
#[allow(unused_imports)]
pub(crate) use oblivion_one::native::scheduler::PipelineWaitReason;
use oblivion_one::native::scheduler::{
    NativeOutputPacingMode, PresentationPipelineView, SchedulerPreparedPrimary,
};

use super::plane::{PlanePageflipIdentity, PresentedPlaneSnapshot};
use crate::native_output::presentation::transaction::FramePresentationReservation;
use crate::native_output::{DirectScanoutCandidateKey, OutputSlotId, OutputTransactionId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TripleCapabilityInputs {
    pub(crate) atomic_kms: bool,
    pub(crate) explicit_swapchain: bool,
    pub(crate) slot_capacity: usize,
    pub(crate) primary_in_fence: bool,
    pub(crate) render_fence_export: bool,
    pub(crate) submission_transport_healthy: bool,
    pub(crate) session_active: bool,
    pub(crate) pool_generation_stable: bool,
    pub(crate) ordinary_vsync: bool,
    pub(crate) swapchain_poisoned: bool,
    pub(crate) software_cursor_visible: bool,
}

pub(crate) const fn derive_triple_capability(inputs: TripleCapabilityInputs) -> TripleCapability {
    if !inputs.atomic_kms {
        TripleCapability::Unavailable(TripleCapabilityBlocker::NonAtomicKms)
    } else if !inputs.explicit_swapchain {
        TripleCapability::Unavailable(TripleCapabilityBlocker::ExplicitSwapchainUnavailable)
    } else if inputs.slot_capacity != 3 {
        TripleCapability::Unavailable(TripleCapabilityBlocker::SlotCapacityMismatch)
    } else if !inputs.primary_in_fence {
        TripleCapability::Unavailable(TripleCapabilityBlocker::PrimaryInFenceUnavailable)
    } else if !inputs.render_fence_export {
        TripleCapability::Unavailable(TripleCapabilityBlocker::RenderFenceExportUnavailable)
    } else if !inputs.submission_transport_healthy {
        TripleCapability::Unavailable(TripleCapabilityBlocker::SubmissionTransportUnhealthy)
    } else if !inputs.session_active {
        TripleCapability::Unavailable(TripleCapabilityBlocker::SessionInactive)
    } else if !inputs.pool_generation_stable {
        TripleCapability::Unavailable(TripleCapabilityBlocker::OutputGenerationUnstable)
    } else if !inputs.ordinary_vsync {
        TripleCapability::Unavailable(TripleCapabilityBlocker::UnsupportedPresentationMode)
    } else if inputs.swapchain_poisoned {
        TripleCapability::Unavailable(TripleCapabilityBlocker::SwapchainPoisoned)
    } else if inputs.software_cursor_visible {
        TripleCapability::Unavailable(TripleCapabilityBlocker::SoftwareCursorVisible)
    } else {
        TripleCapability::Capable
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PresentedPrimaryState {
    Composed {
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
        pageflip: PlanePageflipIdentity,
        slot: OutputSlotId,
        framebuffer_id: u32,
        pool_generation: u64,
        presentation_serial: u64,
    },
    Direct {
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
        pageflip: PlanePageflipIdentity,
        surface_id: u32,
        key: DirectScanoutCandidateKey,
        framebuffer_id: u32,
    },
}

impl PresentedPrimaryState {
    pub(crate) const fn transaction_id(self) -> OutputTransactionId {
        match self {
            Self::Composed { transaction_id, .. } | Self::Direct { transaction_id, .. } => {
                transaction_id
            }
        }
    }

    pub(crate) const fn is_direct(self) -> bool {
        matches!(self, Self::Direct { .. })
    }

    pub(crate) const fn pageflip_identity(self) -> PlanePageflipIdentity {
        match self {
            Self::Composed { pageflip, .. } | Self::Direct { pageflip, .. } => pageflip,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PipelineCommitKind {
    CompositedPrimary {
        transaction_id: OutputTransactionId,
        frame_id: u64,
        slot: OutputSlotId,
        framebuffer_id: u32,
    },
    DirectPrimary {
        transaction_id: OutputTransactionId,
        key: DirectScanoutCandidateKey,
        framebuffer_id: u32,
    },
    PlaneDelta {
        transaction_id: OutputTransactionId,
        cursor_epoch: u64,
        framebuffer_id: Option<u32>,
    },
}

impl PipelineCommitKind {
    pub(crate) const fn transaction_id(self) -> OutputTransactionId {
        match self {
            Self::CompositedPrimary { transaction_id, .. }
            | Self::DirectPrimary { transaction_id, .. }
            | Self::PlaneDelta { transaction_id, .. } => transaction_id,
        }
    }

    pub(crate) const fn is_primary(self) -> bool {
        !matches!(self, Self::PlaneDelta { .. })
    }

    pub(crate) const fn compositor_slot(self) -> Option<OutputSlotId> {
        match self {
            Self::CompositedPrimary { slot, .. } => Some(slot),
            Self::DirectPrimary { .. } | Self::PlaneDelta { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueuedCommitSnapshot {
    pub(crate) token: PageFlipToken,
    pub(crate) output_generation: u64,
    pub(crate) crtc_id: u32,
    pub(crate) reservation: FramePresentationReservation,
    pub(crate) kind: PipelineCommitKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreparedFenceState {
    SubmitWithInFence,
    WaitingForGpu,
    GpuComplete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreparedCompositedState {
    None,
    Rendering {
        slot: OutputSlotId,
        reservation: FramePresentationReservation,
    },
    Ready {
        transaction_id: OutputTransactionId,
        slot: OutputSlotId,
        reservation: FramePresentationReservation,
        fence_state: PreparedFenceState,
    },
    ReadyUnbound {
        transaction_id: OutputTransactionId,
        slot: OutputSlotId,
        fence_state: PreparedFenceState,
    },
}

impl PreparedCompositedState {
    pub(crate) const fn is_present(self) -> bool {
        !matches!(self, Self::None)
    }

    const fn slot(self) -> Option<OutputSlotId> {
        match self {
            Self::None => None,
            Self::Rendering { slot, .. }
            | Self::Ready { slot, .. }
            | Self::ReadyUnbound { slot, .. } => Some(slot),
        }
    }

    const fn reservation(self) -> Option<FramePresentationReservation> {
        match self {
            Self::None => None,
            Self::Rendering { reservation, .. } | Self::Ready { reservation, .. } => {
                Some(reservation)
            }
            Self::ReadyUnbound { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputPipelineSnapshot {
    pub(crate) output_generation: u64,
    pub(crate) future_primary_limit: u8,
    pub(crate) pacing_mode: NativeOutputPacingMode,
    /// Canonical immutable identity of the planes most recently promoted by a
    /// matching pageflip.
    pub(crate) presented_planes: PresentedPlaneSnapshot,
    pub(crate) kernel_submitted: Option<QueuedCommitSnapshot>,
    pub(crate) worker_queued_next: Option<QueuedCommitSnapshot>,
    pub(crate) prepared: PreparedCompositedState,
    pub(crate) free_compositor_slots: u8,
    pub(crate) triple_capability: TripleCapability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PipelineValidationError {
    ZeroOutputGeneration,
    OutputGenerationMismatch {
        transaction_id: OutputTransactionId,
        expected: u64,
        actual: u64,
    },
    DuplicateCommitToken,
    SlotAliasing {
        slot: OutputSlotId,
    },
    FuturePrimaryDepthExceeded {
        depth: u8,
    },
    NonMonotonicTargetOrder {
        earlier_sequence: u64,
        later_sequence: u64,
    },
    TargetGenerationMismatch {
        earlier_generation: u64,
        later_generation: u64,
    },
    KernelSubmittedCapacityExceeded {
        count: u8,
    },
    WorkerQueuedCapacityExceeded {
        count: u8,
    },
    PreparedCapacityExceeded {
        count: u8,
    },
}

pub(crate) fn validate_pipeline_owner_counts(
    kernel_submitted: u8,
    worker_queued_next: u8,
    prepared: u8,
) -> Result<(), PipelineValidationError> {
    if kernel_submitted > 1 {
        return Err(PipelineValidationError::KernelSubmittedCapacityExceeded {
            count: kernel_submitted,
        });
    }
    if worker_queued_next > 1 {
        return Err(PipelineValidationError::WorkerQueuedCapacityExceeded {
            count: worker_queued_next,
        });
    }
    if prepared > 1 {
        return Err(PipelineValidationError::PreparedCapacityExceeded { count: prepared });
    }
    Ok(())
}

impl OutputPipelineSnapshot {
    pub(crate) fn future_primary_depth(&self) -> u8 {
        let queued = [self.kernel_submitted, self.worker_queued_next]
            .into_iter()
            .flatten()
            .filter(|commit| commit.kind.is_primary())
            .count() as u8;
        queued.saturating_add(u8::from(matches!(
            self.prepared,
            PreparedCompositedState::Ready { .. }
        )))
    }

    pub(crate) const fn worker_queue_occupied(&self) -> bool {
        self.worker_queued_next.is_some()
    }

    pub(crate) fn kernel_plane_delta(&self) -> bool {
        self.kernel_submitted
            .is_some_and(|commit| matches!(commit.kind, PipelineCommitKind::PlaneDelta { .. }))
    }

    pub(crate) const fn direct_active(&self) -> bool {
        matches!(
            self.presented_planes.primary,
            Some(PresentedPrimaryState::Direct { .. })
        )
    }

    pub(crate) fn can_render_composed(&self) -> bool {
        !self.direct_active()
            && !self.prepared.is_present()
            && self.future_primary_depth() < self.future_primary_limit
            && self.free_compositor_slots > 0
    }

    pub(crate) fn can_pre_admit_primary(&self) -> bool {
        matches!(
            self.prepared,
            PreparedCompositedState::Ready {
                reservation: FramePresentationReservation::Fixed(_),
                ..
            }
        ) && self.worker_queued_next.is_none()
            && self.future_primary_depth() <= self.future_primary_limit
            && !self.kernel_plane_delta()
    }

    pub(crate) fn validate(&self) -> Result<(), PipelineValidationError> {
        validate_pipeline_owner_counts(
            u8::from(self.kernel_submitted.is_some()),
            u8::from(self.worker_queued_next.is_some()),
            u8::from(self.prepared.is_present()),
        )?;
        if self.output_generation == 0 {
            return Err(PipelineValidationError::ZeroOutputGeneration);
        }
        for commit in [self.kernel_submitted, self.worker_queued_next]
            .into_iter()
            .flatten()
        {
            if commit.output_generation != self.output_generation {
                return Err(PipelineValidationError::OutputGenerationMismatch {
                    transaction_id: commit.kind.transaction_id(),
                    expected: self.output_generation,
                    actual: commit.output_generation,
                });
            }
        }
        if self
            .kernel_submitted
            .zip(self.worker_queued_next)
            .is_some_and(|(kernel, worker)| kernel.token == worker.token)
        {
            return Err(PipelineValidationError::DuplicateCommitToken);
        }

        let occupied = [
            match self.presented_planes.primary {
                Some(PresentedPrimaryState::Composed { slot, .. }) => Some(slot),
                _ => None,
            },
            self.kernel_submitted
                .and_then(|commit| commit.kind.compositor_slot()),
            self.worker_queued_next
                .and_then(|commit| commit.kind.compositor_slot()),
            self.prepared.slot(),
        ];
        for (index, slot) in occupied.iter().copied().enumerate() {
            if let Some(slot) = slot
                && occupied[index + 1..].contains(&Some(slot))
            {
                return Err(PipelineValidationError::SlotAliasing { slot });
            }
        }

        let depth = self.future_primary_depth();
        if depth > 2 {
            return Err(PipelineValidationError::FuturePrimaryDepthExceeded { depth });
        }
        let reservations = [
            self.kernel_submitted
                .filter(|commit| commit.kind.is_primary())
                .map(|commit| commit.reservation),
            self.worker_queued_next
                .filter(|commit| commit.kind.is_primary())
                .map(|commit| commit.reservation),
            self.prepared.reservation(),
        ];
        // Compare adjacent physical owners, including across an empty worker
        // slot. Fixed-to-fixed retains its timestamp ordering guard. Any pair
        // involving VRR is ordered by the phase-free opportunity identity.
        let reservations = reservations.into_iter().flatten();
        for (earlier, later) in reservations.clone().zip(reservations.skip(1)) {
            validate_reservation_order(earlier, later)?;
        }
        Ok(())
    }
}

impl PresentationPipelineView for OutputPipelineSnapshot {
    fn future_primary_limit(&self) -> u8 {
        self.future_primary_limit
    }

    fn kernel_commit_occupied(&self) -> bool {
        self.kernel_submitted.is_some()
    }

    fn kernel_primary_submitted(&self) -> bool {
        self.kernel_submitted
            .is_some_and(|commit| commit.kind.is_primary())
    }

    fn worker_commit_occupied(&self) -> bool {
        self.worker_queued_next.is_some()
    }

    fn worker_primary_queued(&self) -> bool {
        self.worker_queued_next
            .is_some_and(|commit| commit.kind.is_primary())
    }

    fn prepared_primary(&self) -> SchedulerPreparedPrimary {
        match self.prepared {
            PreparedCompositedState::None => SchedulerPreparedPrimary::None,
            PreparedCompositedState::Rendering { .. } => SchedulerPreparedPrimary::Rendering,
            PreparedCompositedState::Ready {
                reservation: FramePresentationReservation::Fixed(target),
                ..
            } => SchedulerPreparedPrimary::Ready { target },
            PreparedCompositedState::Ready {
                reservation: FramePresentationReservation::Variable(_),
                ..
            } => SchedulerPreparedPrimary::ReadyVariable,
            PreparedCompositedState::Ready {
                reservation: FramePresentationReservation::DeferredO1(_),
                ..
            } => SchedulerPreparedPrimary::ReadyUnbound,
            PreparedCompositedState::ReadyUnbound { .. } => SchedulerPreparedPrimary::ReadyUnbound,
        }
    }

    fn free_compositor_slots(&self) -> u8 {
        self.free_compositor_slots
    }

    fn future_primary_depth(&self) -> u8 {
        OutputPipelineSnapshot::future_primary_depth(self)
    }

    fn direct_active(&self) -> bool {
        OutputPipelineSnapshot::direct_active(self)
    }

    fn triple_capable(&self) -> bool {
        matches!(self.triple_capability, TripleCapability::Capable)
    }
}

fn validate_reservation_order(
    earlier: FramePresentationReservation,
    later: FramePresentationReservation,
) -> Result<(), PipelineValidationError> {
    if let (
        FramePresentationReservation::Fixed(earlier_target),
        FramePresentationReservation::Fixed(later_target),
    ) = (earlier, later)
    {
        let earlier_claim = earlier_target.physical_claim();
        let later_claim = later_target.physical_claim();
        if earlier_target.clock_generation != later_target.clock_generation
            || earlier_claim.clock_generation != earlier_target.clock_generation
            || later_claim.clock_generation != later_target.clock_generation
        {
            return Err(PipelineValidationError::TargetGenerationMismatch {
                earlier_generation: earlier_target.clock_generation,
                later_generation: later_target.clock_generation,
            });
        }
        if later_claim.sequence <= earlier_claim.sequence
            || later_claim.presentation_time <= earlier_claim.presentation_time
        {
            return Err(PipelineValidationError::NonMonotonicTargetOrder {
                earlier_sequence: earlier_claim.sequence,
                later_sequence: later_claim.sequence,
            });
        }
        return Ok(());
    }

    let earlier_id = earlier
        .physical_opportunity()
        .map(|opportunity| opportunity.id());
    let later_id = later
        .physical_opportunity()
        .map(|opportunity| opportunity.id());
    match (earlier_id, later_id) {
        (Some(earlier_id), Some(later_id))
            if earlier_id.clock_generation() != later_id.clock_generation() =>
        {
            Err(PipelineValidationError::TargetGenerationMismatch {
                earlier_generation: earlier_id.clock_generation(),
                later_generation: later_id.clock_generation(),
            })
        }
        (Some(earlier_id), Some(later_id)) if later_id.sequence() <= earlier_id.sequence() => {
            Err(PipelineValidationError::NonMonotonicTargetOrder {
                earlier_sequence: earlier_id.sequence(),
                later_sequence: later_id.sequence(),
            })
        }
        (Some(_), Some(_)) => Ok(()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod variable_reservation_tests {
    use super::*;
    use crate::native_output::presentation::transaction::VariablePresentationReservation;
    use oblivion_one::native::buffering::{PresentationOpportunity, PresentationOpportunityId};
    use oblivion_one::native::presentation_deadline::{
        MonotonicTimestampNs, PresentationTarget, PresentationTargetReason, PrimaryRefreshClaim,
        TargetSelectionEvidence,
    };
    use oblivion_one::native::presentation_timing::PresentationTiming;
    use std::num::NonZeroU64;

    fn variable(sequence: u64) -> FramePresentationReservation {
        FramePresentationReservation::Variable(
            VariablePresentationReservation::try_new(
                PresentationOpportunity::new(
                    PresentationOpportunityId::new(1, sequence),
                    PresentationTiming::VrrWindow {
                        anchor: MonotonicTimestampNs::new(1_000_000),
                        earliest_present: MonotonicTimestampNs::new(1_006_000),
                        presentation_not_before: None,
                        preferred_present: MonotonicTimestampNs::new(1_009_000),
                        service_deadline: None,
                        shortest_interval: std::time::Duration::from_millis(6),
                        longest_interval: None,
                    },
                ),
                PresentationTargetReason::ReactiveDouble,
            )
            .expect("VRR opportunity creates variable reservation"),
        )
    }

    fn transaction_id(value: u64) -> OutputTransactionId {
        OutputTransactionId::new(NonZeroU64::new(value).unwrap())
    }

    fn fixed(sequence: u64, presentation_ns: u64) -> FramePresentationReservation {
        let presentation_time = MonotonicTimestampNs::new(presentation_ns);
        FramePresentationReservation::Fixed(PresentationTarget {
            sequence,
            presentation_time,
            submit_not_before: presentation_time,
            render_start_deadline: presentation_time,
            refresh_interval: std::time::Duration::from_millis(6),
            reason: PresentationTargetReason::ReactiveDouble,
            clock_generation: 1,
            estimated: false,
            predicted_unreachable: false,
            physical_claim: PrimaryRefreshClaim {
                sequence,
                presentation_time,
                clock_generation: 1,
            },
            selection_evidence: TargetSelectionEvidence::default(),
        })
    }

    #[test]
    fn ready_and_pending_variable_owners_count_as_real_future_primaries() {
        let ready = OutputPipelineSnapshot {
            output_generation: 1,
            future_primary_limit: 1,
            pacing_mode: NativeOutputPacingMode::ReactiveDouble,
            presented_planes: PresentedPlaneSnapshot::initial(
                super::super::plane::PresentedCursorState::hidden(),
            ),
            kernel_submitted: None,
            worker_queued_next: None,
            prepared: PreparedCompositedState::Ready {
                transaction_id: transaction_id(1),
                slot: OutputSlotId::new(1).unwrap(),
                reservation: variable(1),
                fence_state: PreparedFenceState::SubmitWithInFence,
            },
            free_compositor_slots: 2,
            triple_capability: TripleCapability::Capable,
        };
        assert_eq!(
            ready.prepared_primary(),
            SchedulerPreparedPrimary::ReadyVariable
        );
        assert_eq!(ready.future_primary_depth(), 1);
        assert!(!ready.can_render_composed());
        assert!(!ready.can_pre_admit_primary());
        ready
            .validate()
            .expect("Variable READY ownership validates");

        let rendering = OutputPipelineSnapshot {
            prepared: PreparedCompositedState::Rendering {
                slot: OutputSlotId::new(1).unwrap(),
                reservation: variable(1),
            },
            ..ready
        };
        assert_eq!(
            rendering.prepared_primary(),
            SchedulerPreparedPrimary::Rendering
        );
        rendering
            .validate()
            .expect("Variable rendering ownership validates");

        let pending = OutputPipelineSnapshot {
            prepared: PreparedCompositedState::None,
            kernel_submitted: Some(QueuedCommitSnapshot {
                token: PageFlipToken::new(2).unwrap(),
                output_generation: 1,
                crtc_id: 3,
                reservation: variable(1),
                kind: PipelineCommitKind::CompositedPrimary {
                    transaction_id: transaction_id(2),
                    frame_id: 2,
                    slot: OutputSlotId::new(2).unwrap(),
                    framebuffer_id: 2,
                },
            }),
            ..ready
        };
        assert_eq!(pending.future_primary_depth(), 1);
        assert!(!pending.can_render_composed());
        pending
            .validate()
            .expect("Variable pending ownership validates");
    }

    #[test]
    fn mixed_reservation_order_uses_opportunity_identity_while_fixed_order_keeps_time_guard() {
        let fixed_earlier = fixed(1, 12_000_000);
        let variable_later = variable(2);
        assert!(
            super::validate_reservation_order(fixed_earlier, variable_later).is_ok(),
            "a later Variable opportunity may have a preferred time before a Fixed phase"
        );

        let variable_earlier = variable(1);
        let fixed_later = fixed(2, 3_000_000);
        assert!(
            super::validate_reservation_order(variable_earlier, fixed_later).is_ok(),
            "Variable-to-Fixed order uses generation and sequence only"
        );

        assert!(matches!(
            super::validate_reservation_order(fixed_earlier, fixed(2, 11_000_000)),
            Err(PipelineValidationError::NonMonotonicTargetOrder { .. })
        ));
    }
}
