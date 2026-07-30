#![allow(dead_code)]

use oblivion_one::native::kms::PageFlipToken;
use oblivion_one::native::presentation_deadline::PresentationTarget;
#[allow(unused_imports)]
pub(crate) use oblivion_one::native::scheduler::PipelineWaitReason;
use oblivion_one::native::scheduler::{
    NativeOutputPacingMode, PresentationPipelineView, SchedulerPreparedPrimary,
};

use crate::native_output::{DirectScanoutCandidateKey, OutputSlotId, OutputTransactionId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfirmedPrimaryState {
    Composed {
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
        slot: OutputSlotId,
    },
    Direct {
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
        surface_id: u32,
        key: DirectScanoutCandidateKey,
        framebuffer_id: u32,
    },
}

impl ConfirmedPrimaryState {
    pub(crate) const fn is_direct(self) -> bool {
        matches!(self, Self::Direct { .. })
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
    CursorOnly {
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
            | Self::CursorOnly { transaction_id, .. } => transaction_id,
        }
    }

    pub(crate) const fn is_primary(self) -> bool {
        !matches!(self, Self::CursorOnly { .. })
    }

    pub(crate) const fn compositor_slot(self) -> Option<OutputSlotId> {
        match self {
            Self::CompositedPrimary { slot, .. } => Some(slot),
            Self::DirectPrimary { .. } | Self::CursorOnly { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueuedCommitSnapshot {
    pub(crate) token: PageFlipToken,
    pub(crate) output_generation: u64,
    pub(crate) crtc_id: u32,
    pub(crate) target: PresentationTarget,
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
        target: PresentationTarget,
    },
    Ready {
        transaction_id: OutputTransactionId,
        slot: OutputSlotId,
        target: PresentationTarget,
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
            Self::Rendering { slot, .. } | Self::Ready { slot, .. } => Some(slot),
        }
    }

    const fn target(self) -> Option<PresentationTarget> {
        match self {
            Self::None => None,
            Self::Rendering { target, .. } | Self::Ready { target, .. } => Some(target),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TripleCapabilityBlocker {
    NonAtomicKms,
    ExplicitSwapchainUnavailable,
    SlotCapacityMismatch,
    PrimaryInFenceUnavailable,
    RenderFenceExportUnavailable,
    SubmissionTransportUnhealthy,
    SessionInactive,
    OutputGenerationUnstable,
    UnsupportedPresentationMode,
    SwapchainPoisoned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TripleCapability {
    Capable,
    Unavailable(TripleCapabilityBlocker),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputPipelineSnapshot {
    pub(crate) output_generation: u64,
    pub(crate) pacing_mode: NativeOutputPacingMode,
    pub(crate) current_primary: Option<ConfirmedPrimaryState>,
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
    ReactiveDoubleOwnsPreparedWithQueuedPrimary,
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
        queued.saturating_add(u8::from(self.prepared.is_present()))
    }

    pub(crate) const fn worker_queue_occupied(&self) -> bool {
        self.worker_queued_next.is_some()
    }

    pub(crate) const fn direct_active(&self) -> bool {
        matches!(
            self.current_primary,
            Some(ConfirmedPrimaryState::Direct { .. })
        )
    }

    pub(crate) fn can_render_composed(&self) -> bool {
        !self.direct_active()
            && !self.prepared.is_present()
            && self.future_primary_depth() < 2
            && self.free_compositor_slots > 0
    }

    pub(crate) fn can_pre_admit_primary(&self) -> bool {
        matches!(self.prepared, PreparedCompositedState::Ready { .. })
            && self.worker_queued_next.is_none()
            && self.future_primary_depth() <= 2
            && !self
                .kernel_submitted
                .is_some_and(|commit| matches!(commit.kind, PipelineCommitKind::CursorOnly { .. }))
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

        let mut occupied = Vec::with_capacity(4);
        if let Some(ConfirmedPrimaryState::Composed { slot, .. }) = self.current_primary {
            occupied.push(slot);
        }
        occupied.extend(
            [self.kernel_submitted, self.worker_queued_next]
                .into_iter()
                .flatten()
                .filter_map(|commit| commit.kind.compositor_slot()),
        );
        occupied.extend(self.prepared.slot());
        for (index, slot) in occupied.iter().copied().enumerate() {
            if occupied[index + 1..].contains(&slot) {
                return Err(PipelineValidationError::SlotAliasing { slot });
            }
        }

        let depth = self.future_primary_depth();
        if depth > 2 {
            return Err(PipelineValidationError::FuturePrimaryDepthExceeded { depth });
        }
        if self.pacing_mode == NativeOutputPacingMode::ReactiveDouble
            && self.prepared.is_present()
            && [self.kernel_submitted, self.worker_queued_next]
                .into_iter()
                .flatten()
                .any(|commit| commit.kind.is_primary())
        {
            return Err(PipelineValidationError::ReactiveDoubleOwnsPreparedWithQueuedPrimary);
        }

        let mut targets = [self.kernel_submitted, self.worker_queued_next]
            .into_iter()
            .flatten()
            .filter(|commit| commit.kind.is_primary())
            .map(|commit| commit.target)
            .collect::<Vec<_>>();
        targets.extend(self.prepared.target());
        for pair in targets.windows(2) {
            let [earlier, later] = pair else {
                continue;
            };
            if earlier.clock_generation != later.clock_generation {
                return Err(PipelineValidationError::TargetGenerationMismatch {
                    earlier_generation: earlier.clock_generation,
                    later_generation: later.clock_generation,
                });
            }
            if later.sequence <= earlier.sequence {
                return Err(PipelineValidationError::NonMonotonicTargetOrder {
                    earlier_sequence: earlier.sequence,
                    later_sequence: later.sequence,
                });
            }
        }
        Ok(())
    }
}

impl PresentationPipelineView for OutputPipelineSnapshot {
    fn pacing_mode(&self) -> NativeOutputPacingMode {
        self.pacing_mode
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
            PreparedCompositedState::Ready { target, .. } => {
                SchedulerPreparedPrimary::Ready { target }
            }
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
