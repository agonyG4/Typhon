use super::state_data::ViewportSourceRect;
use super::{BufferIdentity, BufferSize, DmabufBufferHandle, SurfaceCommitSequence};
use wayland_server::protocol::wl_output;

#[derive(Debug, Clone)]
pub struct DirectScanoutSceneCandidate {
    pub surface_id: u32,
    pub root_surface_id: u32,
    pub presented_window_rect: crate::compositor::PresentationRect,
    pub content_epoch: u64,
    pub generation: u64,
    pub surface_presentation_generation: u64,
    pub commit_sequence: SurfaceCommitSequence,
    pub buffer_identity: BufferIdentity,
    pub buffer: DmabufBufferHandle,
    pub buffer_size: BufferSize,
    pub output_size: BufferSize,
    pub viewport_identity_metadata_present: bool,
    pub presentation: crate::compositor::SurfacePresentationMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectScanoutViewportCompatibility {
    pub(crate) identity: bool,
    pub(crate) metadata_present: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectScanoutSceneRejection {
    OwnerMissing,
    OwnerMinimized,
    OwnerDoesNotCoverOutput,
    OwnerRootBufferMissing,
    OwnerTreeHasAdditionalSurface,
    OwnerTreeContentAboveSource,
    NoOutputCoveringApplication,
    EffectRequiresComposition,
    OverlayVisible,
    ApplicationContentAbove,
    ServerSideDecorationVisible,
    PopupVisible,
    NonDmabuf,
    FormatNotProvenOpaque,
    BufferSizeMismatch,
    BufferScaleUnsupported,
    BufferTransformUnsupported,
    ViewportSourceNonIdentity,
    ViewportDestinationNonIdentity,
    VisualClipPresent,
    PlacementMismatch,
    ResizePreviewActive,
    AnimationTransform,
    PresentationOpacity,
    LifecycleAnimation,
    PendingOrUnpublishedWork,
}

impl DirectScanoutSceneRejection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OwnerMissing => "owner_missing",
            Self::OwnerMinimized => "owner_minimized",
            Self::OwnerDoesNotCoverOutput => "owner_does_not_cover_output",
            Self::OwnerRootBufferMissing => "owner_root_buffer_missing",
            Self::OwnerTreeHasAdditionalSurface => "owner_tree_has_additional_surface",
            Self::OwnerTreeContentAboveSource => "owner_tree_content_above_source",
            Self::NoOutputCoveringApplication => "no_output_covering_application",
            Self::EffectRequiresComposition => "effect_requires_composition",
            Self::OverlayVisible => "overlay_visible",
            Self::ApplicationContentAbove => "application_content_above",
            Self::ServerSideDecorationVisible => "server_side_decoration_visible",
            Self::PopupVisible => "popup_visible",
            Self::NonDmabuf => "non_dmabuf",
            Self::FormatNotProvenOpaque => "format_not_proven_opaque",
            Self::BufferSizeMismatch => "buffer_size_mismatch",
            Self::BufferScaleUnsupported => "buffer_scale_unsupported",
            Self::BufferTransformUnsupported => "buffer_transform_unsupported",
            Self::ViewportSourceNonIdentity => "viewport_source_non_identity",
            Self::ViewportDestinationNonIdentity => "viewport_destination_non_identity",
            Self::VisualClipPresent => "visual_clip_present",
            Self::PlacementMismatch => "placement_mismatch",
            Self::ResizePreviewActive => "resize_preview_active",
            Self::AnimationTransform => "animation_transform",
            Self::PresentationOpacity => "presentation_opacity",
            Self::LifecycleAnimation => "lifecycle_animation",
            Self::PendingOrUnpublishedWork => "pending_or_unpublished_work",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirectScanoutSceneBlockers {
    reasons: Vec<DirectScanoutSceneRejection>,
}

impl DirectScanoutSceneBlockers {
    pub const fn is_empty(&self) -> bool {
        self.reasons.is_empty()
    }

    pub fn reasons(&self) -> &[DirectScanoutSceneRejection] {
        &self.reasons
    }

    pub fn primary(&self) -> Option<DirectScanoutSceneRejection> {
        self.reasons.first().copied()
    }

    pub(crate) fn push(&mut self, reason: DirectScanoutSceneRejection) {
        if !self.reasons.contains(&reason) {
            self.reasons.push(reason);
        }
    }
}

pub(crate) fn direct_scanout_viewport_compatibility(
    buffer_size: BufferSize,
    output_size: BufferSize,
    buffer_scale: u32,
    buffer_transform: wl_output::Transform,
    viewport_source: Option<ViewportSourceRect>,
    viewport_destination: Option<BufferSize>,
) -> Result<DirectScanoutViewportCompatibility, DirectScanoutSceneRejection> {
    if buffer_size != output_size {
        return Err(DirectScanoutSceneRejection::BufferSizeMismatch);
    }
    if buffer_scale != 1 {
        return Err(DirectScanoutSceneRejection::BufferScaleUnsupported);
    }
    if buffer_transform != wl_output::Transform::Normal {
        return Err(DirectScanoutSceneRejection::BufferTransformUnsupported);
    }

    if let Some(source) = viewport_source {
        let identity = source.x.is_finite()
            && source.y.is_finite()
            && source.width.is_finite()
            && source.height.is_finite()
            && source.x == 0.0
            && source.y == 0.0
            && source.width == f64::from(buffer_size.width)
            && source.height == f64::from(buffer_size.height);
        if !identity {
            return Err(DirectScanoutSceneRejection::ViewportSourceNonIdentity);
        }
    }
    if let Some(destination) = viewport_destination
        && destination != output_size
    {
        return Err(DirectScanoutSceneRejection::ViewportDestinationNonIdentity);
    }

    Ok(DirectScanoutViewportCompatibility {
        identity: true,
        metadata_present: viewport_source.is_some() || viewport_destination.is_some(),
    })
}

#[cfg(test)]
pub(crate) const fn direct_scanout_scene_rejection_for_flags(
    overlays_visible: bool,
    popup_visible: bool,
) -> Option<DirectScanoutSceneRejection> {
    if overlays_visible {
        Some(DirectScanoutSceneRejection::OverlayVisible)
    } else if popup_visible {
        Some(DirectScanoutSceneRejection::PopupVisible)
    } else {
        None
    }
}

pub(crate) const fn direct_scanout_scene_rejection_for_effects(
    summary: crate::compositor::EffectSceneSummary,
) -> Option<DirectScanoutSceneRejection> {
    if summary.requires_composition && summary.visible_instance_count > 0 {
        Some(DirectScanoutSceneRejection::EffectRequiresComposition)
    } else {
        None
    }
}
