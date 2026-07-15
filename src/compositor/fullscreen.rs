use super::{BufferIdentity, BufferSize, DmabufBufferHandle, SurfaceCommitSequence};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FullscreenPresentationState {
    pub owner_root_surface_id: u32,
    pub output_width: u32,
    pub output_height: u32,
}

#[derive(Debug, Clone)]
pub struct DirectScanoutSceneCandidate {
    pub surface_id: u32,
    pub root_surface_id: u32,
    pub generation: u64,
    pub commit_sequence: SurfaceCommitSequence,
    pub buffer_identity: BufferIdentity,
    pub buffer: DmabufBufferHandle,
    pub buffer_size: BufferSize,
    pub output_size: BufferSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectScanoutSceneRejection {
    NoFullscreenOwner,
    OwnerMissing,
    OwnerMinimized,
    OwnerDoesNotCoverOutput,
    OwnerRootBufferMissing,
    OwnerTreeHasAdditionalSurface,
    OverlayVisible,
    PopupVisible,
    NonDmabuf,
    FormatNotOpaqueXrgb8888,
    BufferSizeMismatch,
    BufferScaleUnsupported,
    BufferTransformUnsupported,
    ViewportUnsupported,
    VisualClipPresent,
    PlacementMismatch,
    ResizePreviewActive,
    PendingOrUnpublishedWork,
}

impl DirectScanoutSceneRejection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoFullscreenOwner => "no_fullscreen_owner",
            Self::OwnerMissing => "owner_missing",
            Self::OwnerMinimized => "owner_minimized",
            Self::OwnerDoesNotCoverOutput => "owner_does_not_cover_output",
            Self::OwnerRootBufferMissing => "owner_root_buffer_missing",
            Self::OwnerTreeHasAdditionalSurface => "owner_tree_has_additional_surface",
            Self::OverlayVisible => "overlay_visible",
            Self::PopupVisible => "popup_visible",
            Self::NonDmabuf => "non_dmabuf",
            Self::FormatNotOpaqueXrgb8888 => "format_not_opaque_xrgb8888",
            Self::BufferSizeMismatch => "buffer_size_mismatch",
            Self::BufferScaleUnsupported => "buffer_scale_unsupported",
            Self::BufferTransformUnsupported => "buffer_transform_unsupported",
            Self::ViewportUnsupported => "viewport_unsupported",
            Self::VisualClipPresent => "visual_clip_present",
            Self::PlacementMismatch => "placement_mismatch",
            Self::ResizePreviewActive => "resize_preview_active",
            Self::PendingOrUnpublishedWork => "pending_or_unpublished_work",
        }
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullscreenPresentationRejection {
    NoFullscreenOwner,
    OwnerMissing,
    OwnerMinimized,
    OwnerDoesNotCoverOutput,
    OwnerOpacityUnknown,
    OverlayVisible,
    SoftwareCursorVisible,
    TransformOrScaleIncompatible,
}

impl FullscreenPresentationRejection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoFullscreenOwner => "no_fullscreen_owner",
            Self::OwnerMissing => "owner_missing",
            Self::OwnerMinimized => "owner_minimized",
            Self::OwnerDoesNotCoverOutput => "owner_does_not_cover_output",
            Self::OwnerOpacityUnknown => "owner_opacity_unknown",
            Self::OverlayVisible => "overlay_visible",
            Self::SoftwareCursorVisible => "software_cursor_visible",
            Self::TransformOrScaleIncompatible => "transform_or_scale_incompatible",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FullscreenPresentationEligibility {
    pub owner: Option<FullscreenPresentationState>,
    pub eligible: bool,
    pub rejection: Option<FullscreenPresentationRejection>,
    pub fully_opaque: bool,
    pub exactly_covers_output: bool,
    pub overlays_visible: bool,
    pub software_cursor_visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FullscreenRenderPlanMetrics {
    pub fullscreen_active: bool,
    pub owner_root_surface_id: Option<u32>,
    pub solitary_tree_active: bool,
    pub culled_surface_count: usize,
    pub wallpaper_culled: bool,
    pub visible_overlay_count: usize,
    pub rejection: Option<FullscreenPresentationRejection>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_scanout_scene_rejection_prioritizes_visible_overlay() {
        assert_eq!(
            direct_scanout_scene_rejection_for_flags(true, true),
            Some(DirectScanoutSceneRejection::OverlayVisible)
        );
    }
}
