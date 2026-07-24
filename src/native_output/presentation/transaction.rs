#![allow(dead_code)]

use std::{collections::HashMap, fmt, num::NonZeroU64};

use oblivion_one::compositor::{CompositorFrameBatchId, DirectScanoutSceneCandidate};
use oblivion_one::native::presentation_deadline::{MonotonicTimestampNs, PresentationTarget};
use oblivion_one::native::scheduler::NativeOutputPacingMode;

use crate::native_output::scanout::OutputSlotId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ContentEpochId(NonZeroU64);

impl ContentEpochId {
    pub(crate) const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    #[allow(dead_code)]
    pub(crate) const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct OutputTransactionId(NonZeroU64);

impl OutputTransactionId {
    #[cfg(test)]
    pub(crate) const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    pub(crate) const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputTransactionAllocator {
    next: NonZeroU64,
    exhausted: bool,
}

impl Default for OutputTransactionAllocator {
    fn default() -> Self {
        Self {
            next: NonZeroU64::MIN,
            exhausted: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputTransactionAllocationError {
    Exhausted,
}

impl fmt::Display for OutputTransactionAllocationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for OutputTransactionAllocationError {}

impl OutputTransactionAllocator {
    pub(crate) fn allocate(
        &mut self,
    ) -> Result<OutputTransactionId, OutputTransactionAllocationError> {
        if self.exhausted {
            return Err(OutputTransactionAllocationError::Exhausted);
        }
        let id = OutputTransactionId(self.next);
        if id.0.get() == u64::MAX {
            self.exhausted = true;
        } else {
            self.next = NonZeroU64::new(id.0.get() + 1).expect("increment remains nonzero");
        }
        Ok(id)
    }

    #[cfg(test)]
    pub(crate) const fn with_next(next: NonZeroU64) -> Self {
        Self {
            next,
            exhausted: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputTransactionBuildError {
    ZeroOutputGeneration,
    MissingFrameBatch,
    MissingDirectSurface,
    DirectPrimaryForCompositedContent,
    CompositorPrimaryForDirectContent,
    ChangedPrimaryForCursorOnly,
    FrameBatchForCursorOnly,
    DirectSurfaceForCompositedContent,
    DirectSurfaceForCursorOnly,
    OverlayAssignmentsUnsupported,
}

impl fmt::Display for OutputTransactionBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for OutputTransactionBuildError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputTransactionContent {
    Composited {
        frame_id: u64,
        render_generation: u64,
        pool_generation: u64,
    },
    Direct {
        frame_id: u64,
        key: DirectScanoutCandidateKey,
    },
    CursorOnly {
        cursor_epoch: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrimaryPlaneAssignment {
    CompositorFramebuffer {
        slot: OutputSlotId,
        framebuffer_id: u32,
    },
    CompatibilityFramebuffer {
        framebuffer_id: u32,
    },
    ClientFramebuffer {
        key: DirectScanoutCandidateKey,
        framebuffer_id: u32,
    },
    Unchanged,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorPlaneAssignment {
    Atomic {
        desired_epoch: u64,
        framebuffer_id: Option<u32>,
        visible: bool,
    },
    Unchanged,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OverlayPlaneAssignment {
    pub(crate) plane_id: u32,
    pub(crate) framebuffer_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutputPlanePlan {
    primary: PrimaryPlaneAssignment,
    cursor: CursorPlaneAssignment,
    overlays: Vec<OverlayPlaneAssignment>,
}

impl OutputPlanePlan {
    pub(crate) fn new(
        primary: PrimaryPlaneAssignment,
        cursor: CursorPlaneAssignment,
        overlays: Vec<OverlayPlaneAssignment>,
    ) -> Result<Self, OutputTransactionBuildError> {
        if !overlays.is_empty() {
            return Err(OutputTransactionBuildError::OverlayAssignmentsUnsupported);
        }
        Ok(Self {
            primary,
            cursor,
            overlays,
        })
    }

    pub(crate) const fn primary(&self) -> PrimaryPlaneAssignment {
        self.primary
    }

    pub(crate) const fn cursor(&self) -> CursorPlaneAssignment {
        self.cursor
    }

    pub(crate) fn overlays(&self) -> &[OverlayPlaneAssignment] {
        &self.overlays
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputAcquirePlan {
    RenderFence,
    ClientContentAlreadyReady,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputReleasePlan {
    Pageflip,
    OutFenceThenPageflip,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputSynchronizationPlan {
    acquire: OutputAcquirePlan,
    release: OutputReleasePlan,
}

impl OutputSynchronizationPlan {
    pub(crate) const fn new(acquire: OutputAcquirePlan, release: OutputReleasePlan) -> Self {
        Self { acquire, release }
    }

    pub(crate) const fn acquire(self) -> OutputAcquirePlan {
        self.acquire
    }

    pub(crate) const fn release(self) -> OutputReleasePlan {
        self.release
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputProtocolObligations {
    frame_batch_id: Option<CompositorFrameBatchId>,
    direct_surface_id: Option<u32>,
}

impl OutputProtocolObligations {
    pub(crate) const fn composited(frame_batch_id: CompositorFrameBatchId) -> Self {
        Self {
            frame_batch_id: Some(frame_batch_id),
            direct_surface_id: None,
        }
    }

    pub(crate) const fn direct(
        frame_batch_id: CompositorFrameBatchId,
        direct_surface_id: u32,
    ) -> Self {
        Self {
            frame_batch_id: Some(frame_batch_id),
            direct_surface_id: Some(direct_surface_id),
        }
    }

    pub(crate) const fn cursor_only() -> Self {
        Self {
            frame_batch_id: None,
            direct_surface_id: None,
        }
    }

    pub(crate) const fn frame_batch_id(self) -> Option<CompositorFrameBatchId> {
        self.frame_batch_id
    }

    pub(crate) const fn direct_surface_id(self) -> Option<u32> {
        self.direct_surface_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutputTransaction {
    id: OutputTransactionId,
    output_generation: u64,
    created_at: MonotonicTimestampNs,
    target: PresentationTarget,
    pacing_mode: NativeOutputPacingMode,
    content: OutputTransactionContent,
    planes: OutputPlanePlan,
    synchronization: OutputSynchronizationPlan,
    obligations: OutputProtocolObligations,
}

impl OutputTransaction {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn composited(
        id: OutputTransactionId,
        output_generation: u64,
        created_at: MonotonicTimestampNs,
        target: PresentationTarget,
        pacing_mode: NativeOutputPacingMode,
        frame_id: u64,
        render_generation: u64,
        pool_generation: u64,
        slot: OutputSlotId,
        framebuffer_id: u32,
        cursor: Option<CursorPlaneAssignment>,
        frame_batch_id: CompositorFrameBatchId,
    ) -> Result<Self, OutputTransactionBuildError> {
        Self::build(
            id,
            output_generation,
            created_at,
            target,
            pacing_mode,
            OutputTransactionContent::Composited {
                frame_id,
                render_generation,
                pool_generation,
            },
            OutputPlanePlan::new(
                PrimaryPlaneAssignment::CompositorFramebuffer {
                    slot,
                    framebuffer_id,
                },
                cursor.unwrap_or(CursorPlaneAssignment::Unchanged),
                Vec::new(),
            )?,
            OutputSynchronizationPlan::new(
                OutputAcquirePlan::RenderFence,
                OutputReleasePlan::OutFenceThenPageflip,
            ),
            OutputProtocolObligations::composited(frame_batch_id),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn direct(
        id: OutputTransactionId,
        output_generation: u64,
        created_at: MonotonicTimestampNs,
        target: PresentationTarget,
        pacing_mode: NativeOutputPacingMode,
        frame_id: u64,
        key: DirectScanoutCandidateKey,
        framebuffer_id: u32,
        cursor: Option<CursorPlaneAssignment>,
        frame_batch_id: CompositorFrameBatchId,
        direct_surface_id: u32,
        release: OutputReleasePlan,
    ) -> Result<Self, OutputTransactionBuildError> {
        Self::build(
            id,
            output_generation,
            created_at,
            target,
            pacing_mode,
            OutputTransactionContent::Direct { frame_id, key },
            OutputPlanePlan::new(
                PrimaryPlaneAssignment::ClientFramebuffer {
                    key,
                    framebuffer_id,
                },
                cursor.unwrap_or(CursorPlaneAssignment::Unchanged),
                Vec::new(),
            )?,
            OutputSynchronizationPlan::new(OutputAcquirePlan::ClientContentAlreadyReady, release),
            OutputProtocolObligations::direct(frame_batch_id, direct_surface_id),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn compatibility_composited(
        id: OutputTransactionId,
        output_generation: u64,
        created_at: MonotonicTimestampNs,
        target: PresentationTarget,
        pacing_mode: NativeOutputPacingMode,
        frame_id: u64,
        render_generation: u64,
        framebuffer_id: u32,
        cursor: Option<CursorPlaneAssignment>,
        frame_batch_id: CompositorFrameBatchId,
    ) -> Result<Self, OutputTransactionBuildError> {
        Self::build(
            id,
            output_generation,
            created_at,
            target,
            pacing_mode,
            OutputTransactionContent::Composited {
                frame_id,
                render_generation,
                pool_generation: output_generation,
            },
            OutputPlanePlan::new(
                PrimaryPlaneAssignment::CompatibilityFramebuffer { framebuffer_id },
                cursor.unwrap_or(CursorPlaneAssignment::Unchanged),
                Vec::new(),
            )?,
            OutputSynchronizationPlan::new(OutputAcquirePlan::None, OutputReleasePlan::Pageflip),
            OutputProtocolObligations::composited(frame_batch_id),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn cursor_only(
        id: OutputTransactionId,
        output_generation: u64,
        created_at: MonotonicTimestampNs,
        target: PresentationTarget,
        pacing_mode: NativeOutputPacingMode,
        cursor_epoch: u64,
        framebuffer_id: Option<u32>,
        visible: bool,
        release: OutputReleasePlan,
    ) -> Result<Self, OutputTransactionBuildError> {
        Self::build(
            id,
            output_generation,
            created_at,
            target,
            pacing_mode,
            OutputTransactionContent::CursorOnly { cursor_epoch },
            OutputPlanePlan::new(
                PrimaryPlaneAssignment::Unchanged,
                CursorPlaneAssignment::Atomic {
                    desired_epoch: cursor_epoch,
                    framebuffer_id,
                    visible,
                },
                Vec::new(),
            )?,
            OutputSynchronizationPlan::new(OutputAcquirePlan::None, release),
            OutputProtocolObligations::cursor_only(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        id: OutputTransactionId,
        output_generation: u64,
        created_at: MonotonicTimestampNs,
        target: PresentationTarget,
        pacing_mode: NativeOutputPacingMode,
        content: OutputTransactionContent,
        planes: OutputPlanePlan,
        synchronization: OutputSynchronizationPlan,
        obligations: OutputProtocolObligations,
    ) -> Result<Self, OutputTransactionBuildError> {
        if output_generation == 0 {
            return Err(OutputTransactionBuildError::ZeroOutputGeneration);
        }
        match content {
            OutputTransactionContent::Composited { .. } if obligations.frame_batch_id.is_none() => {
                return Err(OutputTransactionBuildError::MissingFrameBatch);
            }
            OutputTransactionContent::Direct { .. } => {
                if obligations.frame_batch_id.is_none() {
                    return Err(OutputTransactionBuildError::MissingFrameBatch);
                }
                if obligations.direct_surface_id.is_none() {
                    return Err(OutputTransactionBuildError::MissingDirectSurface);
                }
                if matches!(
                    planes.primary,
                    PrimaryPlaneAssignment::CompositorFramebuffer { .. }
                ) {
                    return Err(OutputTransactionBuildError::CompositorPrimaryForDirectContent);
                }
            }
            OutputTransactionContent::CursorOnly { .. } => {
                if obligations.frame_batch_id.is_some() {
                    return Err(OutputTransactionBuildError::FrameBatchForCursorOnly);
                }
                if obligations.direct_surface_id.is_some() {
                    return Err(OutputTransactionBuildError::DirectSurfaceForCursorOnly);
                }
                if !matches!(planes.primary, PrimaryPlaneAssignment::Unchanged) {
                    return Err(OutputTransactionBuildError::ChangedPrimaryForCursorOnly);
                }
            }
            OutputTransactionContent::Composited { .. } => {
                if obligations.direct_surface_id.is_some() {
                    return Err(OutputTransactionBuildError::DirectSurfaceForCompositedContent);
                }
                if matches!(
                    planes.primary,
                    PrimaryPlaneAssignment::ClientFramebuffer { .. }
                ) {
                    return Err(OutputTransactionBuildError::DirectPrimaryForCompositedContent);
                }
            }
        }
        Ok(Self {
            id,
            output_generation,
            created_at,
            target,
            pacing_mode,
            content,
            planes,
            synchronization,
            obligations,
        })
    }

    pub(crate) const fn id(&self) -> OutputTransactionId {
        self.id
    }

    pub(crate) const fn output_generation(&self) -> u64 {
        self.output_generation
    }

    pub(crate) const fn created_at(&self) -> MonotonicTimestampNs {
        self.created_at
    }

    pub(crate) const fn target(&self) -> PresentationTarget {
        self.target
    }

    pub(crate) const fn pacing_mode(&self) -> NativeOutputPacingMode {
        self.pacing_mode
    }

    pub(crate) const fn content(&self) -> OutputTransactionContent {
        self.content
    }

    pub(crate) fn planes(&self) -> &OutputPlanePlan {
        &self.planes
    }

    pub(crate) const fn synchronization(&self) -> OutputSynchronizationPlan {
        self.synchronization
    }

    pub(crate) const fn obligations(&self) -> OutputProtocolObligations {
        self.obligations
    }
}
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContentObservation {
    pub(crate) surface_id: u32,
    pub(crate) buffer_id: NonZeroU64,
    pub(crate) attachment_sequence: u64,
    pub(crate) epoch: ContentEpochId,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct ContentEpochTracker {
    current_by_surface: HashMap<u32, ContentObservation>,
    next: NonZeroU64,
}

impl Default for ContentEpochTracker {
    fn default() -> Self {
        Self {
            current_by_surface: HashMap::new(),
            next: NonZeroU64::MIN,
        }
    }
}

#[allow(dead_code)]
impl ContentEpochTracker {
    pub(crate) fn observe(
        &mut self,
        surface_id: u32,
        buffer_id: NonZeroU64,
        attachment_sequence: u64,
    ) -> ContentObservation {
        let epoch = ContentEpochId(self.next);
        self.next = epoch
            .0
            .get()
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .expect("content epoch identifiers exhausted");
        let observation = ContentObservation {
            surface_id,
            buffer_id,
            attachment_sequence,
            epoch,
        };
        self.current_by_surface.insert(surface_id, observation);
        observation
    }

    pub(crate) fn record_metadata_commit(&self, surface_id: u32) -> Option<ContentEpochId> {
        self.current_by_surface
            .get(&surface_id)
            .map(|observation| observation.epoch)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct OutputContentKey {
    pub(crate) surface_id: u32,
    pub(crate) buffer_id: NonZeroU64,
    pub(crate) content_epoch: ContentEpochId,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: u32,
    pub(crate) modifier: u64,
    pub(crate) transform: u32,
    pub(crate) scale_milli: u32,
    pub(crate) color_epoch: u64,
}

impl OutputContentKey {
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn new(
        surface_id: u32,
        buffer_id: NonZeroU64,
        content_epoch: ContentEpochId,
        width: u32,
        height: u32,
        format: u32,
        modifier: u64,
        transform: u32,
        scale_milli: u32,
        color_epoch: u64,
    ) -> Self {
        Self {
            surface_id,
            buffer_id,
            content_epoch,
            width,
            height,
            format,
            modifier,
            transform,
            scale_milli,
            color_epoch,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DirectScanoutCandidateKey {
    pub(crate) content: OutputContentKey,
    pub(crate) output_generation: u64,
    pub(crate) cursor_plan_key: Option<u64>,
    pub(crate) color_epoch: u64,
}

impl DirectScanoutCandidateKey {
    pub(crate) fn from_candidate(
        candidate: &DirectScanoutSceneCandidate,
        output_generation: u64,
        cursor_plan_key: Option<u64>,
        color_epoch: u64,
    ) -> Option<Self> {
        let buffer_id = NonZeroU64::new(candidate.buffer_identity.id().get())?;
        let modifier = candidate.buffer.planes().first()?.descriptor().modifier.0;
        Some(Self {
            content: OutputContentKey::new(
                candidate.surface_id,
                buffer_id,
                ContentEpochId::new(NonZeroU64::new(candidate.content_epoch)?),
                candidate.buffer_size.width,
                candidate.buffer_size.height,
                candidate.buffer.format().as_fourcc(),
                modifier,
                0,
                1_000,
                color_epoch,
            ),
            output_generation,
            cursor_plan_key,
            color_epoch,
        })
    }
}
