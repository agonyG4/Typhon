#![allow(dead_code)]

use std::{collections::HashMap, fmt, num::NonZeroU64};

use oblivion_one::compositor::{
    CompositorFrameBatchId, DirectScanoutSceneCandidate, DrmContentType, OutputPresentationMode,
    SurfaceDamagePresentation,
};
use oblivion_one::native::kms::AtomicCursorVisualState;
use oblivion_one::native::presentation_deadline::{MonotonicTimestampNs, PresentationTarget};
use oblivion_one::native::scheduler::NativeOutputPacingMode;

use super::async_validation::CompositedAsyncValidationKey;
use super::plane::{CursorSidecarId, PlaneWriteSet};
use crate::native_output::scanout::{CursorContentKey, OutputSlotId};

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
    ChangedPrimaryForPlaneDelta,
    FrameBatchForPlaneDelta,
    DirectSurfaceForCompositedContent,
    DirectSurfaceForPlaneDelta,
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
        equivalent_direct_key: Option<DirectScanoutCandidateKey>,
    },
    Direct {
        frame_id: u64,
        key: DirectScanoutCandidateKey,
    },
    CompatibilityImmediate {
        frame_id: u64,
    },
    PlaneDelta {
        changed: PlaneWriteSet,
        cursor_sidecar_id: CursorSidecarId,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CursorPlaneAssignment {
    Atomic {
        desired_epoch: u64,
        state: Option<AtomicCursorVisualState>,
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

    pub(crate) const fn cursor(&self) -> &CursorPlaneAssignment {
        &self.cursor
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

    pub(crate) const fn plane_delta() -> Self {
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
    presentation_mode: OutputPresentationMode,
    content_type: DrmContentType,
    async_validation_key: Option<CompositedAsyncValidationKey>,
    content: OutputTransactionContent,
    planes: OutputPlanePlan,
    synchronization: OutputSynchronizationPlan,
    obligations: OutputProtocolObligations,
    surface_damage: Option<SurfaceDamagePresentation>,
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
        Self::composited_with_direct_equivalence(
            id,
            output_generation,
            created_at,
            target,
            pacing_mode,
            frame_id,
            render_generation,
            pool_generation,
            slot,
            framebuffer_id,
            cursor,
            frame_batch_id,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn composited_with_direct_equivalence(
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
        equivalent_direct_key: Option<DirectScanoutCandidateKey>,
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
                equivalent_direct_key,
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
                equivalent_direct_key: None,
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
    pub(crate) fn compatibility_immediate(
        id: OutputTransactionId,
        output_generation: u64,
        created_at: MonotonicTimestampNs,
        target: PresentationTarget,
        pacing_mode: NativeOutputPacingMode,
        frame_id: u64,
        frame_batch_id: CompositorFrameBatchId,
    ) -> Result<Self, OutputTransactionBuildError> {
        Self::build(
            id,
            output_generation,
            created_at,
            target,
            pacing_mode,
            OutputTransactionContent::CompatibilityImmediate { frame_id },
            OutputPlanePlan::new(
                PrimaryPlaneAssignment::Disabled,
                CursorPlaneAssignment::Unchanged,
                Vec::new(),
            )?,
            OutputSynchronizationPlan::new(OutputAcquirePlan::None, OutputReleasePlan::None),
            OutputProtocolObligations::composited(frame_batch_id),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn cursor_plane_delta(
        id: OutputTransactionId,
        output_generation: u64,
        created_at: MonotonicTimestampNs,
        target: PresentationTarget,
        pacing_mode: NativeOutputPacingMode,
        cursor_epoch: u64,
        state: Option<AtomicCursorVisualState>,
        release: OutputReleasePlan,
    ) -> Result<Self, OutputTransactionBuildError> {
        let sidecar_id =
            CursorSidecarId::new(NonZeroU64::new(cursor_epoch).expect("cursor epoch is nonzero"));
        Self::plane_delta(
            id,
            output_generation,
            created_at,
            target,
            pacing_mode,
            PlaneWriteSet::CURSOR,
            sidecar_id,
            cursor_epoch,
            state,
            release,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn plane_delta(
        id: OutputTransactionId,
        output_generation: u64,
        created_at: MonotonicTimestampNs,
        target: PresentationTarget,
        pacing_mode: NativeOutputPacingMode,
        changed: PlaneWriteSet,
        cursor_sidecar_id: CursorSidecarId,
        cursor_epoch: u64,
        state: Option<AtomicCursorVisualState>,
        release: OutputReleasePlan,
    ) -> Result<Self, OutputTransactionBuildError> {
        Self::build(
            id,
            output_generation,
            created_at,
            target,
            pacing_mode,
            OutputTransactionContent::PlaneDelta {
                changed,
                cursor_sidecar_id,
            },
            OutputPlanePlan::new(
                PrimaryPlaneAssignment::Unchanged,
                CursorPlaneAssignment::Atomic {
                    desired_epoch: cursor_epoch,
                    state,
                },
                Vec::new(),
            )?,
            OutputSynchronizationPlan::new(OutputAcquirePlan::None, release),
            OutputProtocolObligations::plane_delta(),
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
            OutputTransactionContent::CompatibilityImmediate { .. } => {
                if obligations.frame_batch_id.is_none() {
                    return Err(OutputTransactionBuildError::MissingFrameBatch);
                }
                if obligations.direct_surface_id.is_some() {
                    return Err(OutputTransactionBuildError::DirectSurfaceForCompositedContent);
                }
                if !matches!(planes.primary, PrimaryPlaneAssignment::Disabled) {
                    return Err(OutputTransactionBuildError::DirectPrimaryForCompositedContent);
                }
            }
            OutputTransactionContent::PlaneDelta { changed, .. } => {
                changed
                    .validate_cursor_delta()
                    .map_err(|_| OutputTransactionBuildError::ChangedPrimaryForPlaneDelta)?;
                if obligations.frame_batch_id.is_some() {
                    return Err(OutputTransactionBuildError::FrameBatchForPlaneDelta);
                }
                if obligations.direct_surface_id.is_some() {
                    return Err(OutputTransactionBuildError::DirectSurfaceForPlaneDelta);
                }
                if !matches!(planes.primary, PrimaryPlaneAssignment::Unchanged) {
                    return Err(OutputTransactionBuildError::ChangedPrimaryForPlaneDelta);
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
            presentation_mode: OutputPresentationMode::Vsync,
            content_type: DrmContentType::Graphics,
            async_validation_key: None,
            content,
            planes,
            synchronization,
            obligations,
            surface_damage: None,
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

    pub(crate) const fn presentation_mode(&self) -> OutputPresentationMode {
        self.presentation_mode
    }

    pub(crate) const fn content_type(&self) -> DrmContentType {
        self.content_type
    }

    pub(crate) fn with_presentation_state(
        mut self,
        presentation_mode: OutputPresentationMode,
        content_type: DrmContentType,
    ) -> Self {
        self.presentation_mode = presentation_mode;
        self.content_type = content_type;
        if presentation_mode.is_async() {
            self.pacing_mode = NativeOutputPacingMode::ReactiveDouble;
        }
        self
    }

    pub(crate) fn with_async_validation_key(
        mut self,
        key: Option<CompositedAsyncValidationKey>,
    ) -> Self {
        self.async_validation_key = key;
        self
    }

    pub(crate) fn with_surface_damage(mut self, surface_damage: SurfaceDamagePresentation) -> Self {
        debug_assert!(self.surface_damage.is_none());
        self.surface_damage = Some(surface_damage);
        self
    }

    pub(crate) fn surface_damage(&self) -> Option<&SurfaceDamagePresentation> {
        self.surface_damage.as_ref()
    }

    pub(crate) const fn async_validation_key(&self) -> Option<CompositedAsyncValidationKey> {
        self.async_validation_key
    }

    pub(crate) const fn content(&self) -> OutputTransactionContent {
        self.content
    }

    pub(crate) const fn equivalent_direct_key(&self) -> Option<DirectScanoutCandidateKey> {
        match self.content {
            OutputTransactionContent::Composited {
                equivalent_direct_key,
                ..
            } => equivalent_direct_key,
            OutputTransactionContent::Direct { .. }
            | OutputTransactionContent::CompatibilityImmediate { .. }
            | OutputTransactionContent::PlaneDelta { .. } => None,
        }
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
    pub(crate) cursor_content_key: Option<CursorContentKey>,
    pub(crate) color_epoch: u64,
}

impl DirectScanoutCandidateKey {
    pub(crate) fn from_candidate(
        candidate: &DirectScanoutSceneCandidate,
        output_generation: u64,
        cursor_content_key: Option<CursorContentKey>,
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
            cursor_content_key,
            color_epoch,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectContentDisposition {
    NewContent,
    MatchesPresented,
    MatchesQueuedOrSubmitted,
}

pub(crate) fn classify_direct_content(
    candidate: DirectScanoutCandidateKey,
    presented: Option<DirectScanoutCandidateKey>,
    pending: Option<DirectScanoutCandidateKey>,
) -> DirectContentDisposition {
    if presented == Some(candidate) {
        // A confirmed presented assignment is authoritative when it overlaps
        // with a pending snapshot.
        DirectContentDisposition::MatchesPresented
    } else if pending == Some(candidate) {
        DirectContentDisposition::MatchesQueuedOrSubmitted
    } else {
        DirectContentDisposition::NewContent
    }
}
