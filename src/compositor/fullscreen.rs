use super::state_data::ViewportSourceRect;
use super::{BufferIdentity, BufferSize, DmabufBufferHandle, SurfaceCommitSequence};
use wayland_server::protocol::wl_output;

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
    pub viewport_identity_metadata_present: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectScanoutViewportCompatibility {
    pub(crate) identity: bool,
    pub(crate) metadata_present: bool,
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
    ClientCursorUnsupported,
    NonDmabuf,
    FormatNotOpaqueXrgb8888,
    BufferSizeMismatch,
    BufferScaleUnsupported,
    BufferTransformUnsupported,
    ViewportSourceNonIdentity,
    ViewportDestinationNonIdentity,
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
            Self::ClientCursorUnsupported => "client_cursor_unsupported",
            Self::NonDmabuf => "non_dmabuf",
            Self::FormatNotOpaqueXrgb8888 => "format_not_opaque_xrgb8888",
            Self::BufferSizeMismatch => "buffer_size_mismatch",
            Self::BufferScaleUnsupported => "buffer_scale_unsupported",
            Self::BufferTransformUnsupported => "buffer_transform_unsupported",
            Self::ViewportSourceNonIdentity => "viewport_source_non_identity",
            Self::ViewportDestinationNonIdentity => "viewport_destination_non_identity",
            Self::VisualClipPresent => "visual_clip_present",
            Self::PlacementMismatch => "placement_mismatch",
            Self::ResizePreviewActive => "resize_preview_active",
            Self::PendingOrUnpublishedWork => "pending_or_unpublished_work",
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
    use crate::compositor::state_data::ViewportSourceRect;
    use wayland_server::protocol::wl_output;

    fn output_size() -> BufferSize {
        BufferSize::new(1920, 1080).unwrap()
    }

    fn full_source() -> ViewportSourceRect {
        ViewportSourceRect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        }
    }

    fn classify(
        buffer_size: BufferSize,
        output_size: BufferSize,
        buffer_scale: u32,
        buffer_transform: wl_output::Transform,
        viewport_source: Option<ViewportSourceRect>,
        viewport_destination: Option<BufferSize>,
    ) -> Result<DirectScanoutViewportCompatibility, DirectScanoutSceneRejection> {
        direct_scanout_viewport_compatibility(
            buffer_size,
            output_size,
            buffer_scale,
            buffer_transform,
            viewport_source,
            viewport_destination,
        )
    }

    #[test]
    fn direct_scanout_scene_rejection_prioritizes_visible_overlay() {
        assert_eq!(
            direct_scanout_scene_rejection_for_flags(true, true),
            Some(DirectScanoutSceneRejection::OverlayVisible)
        );
    }

    #[test]
    fn viewport_without_metadata_is_identity() {
        let result = classify(
            output_size(),
            output_size(),
            1,
            wl_output::Transform::Normal,
            None,
            None,
        )
        .unwrap();
        assert!(result.identity);
        assert!(!result.metadata_present);
    }

    #[test]
    fn full_buffer_source_is_identity() {
        let result = classify(
            output_size(),
            output_size(),
            1,
            wl_output::Transform::Normal,
            Some(full_source()),
            None,
        )
        .unwrap();
        assert!(result.identity);
        assert!(result.metadata_present);
    }

    #[test]
    fn output_sized_destination_is_identity() {
        let result = classify(
            output_size(),
            output_size(),
            1,
            wl_output::Transform::Normal,
            None,
            Some(output_size()),
        )
        .unwrap();
        assert!(result.identity);
        assert!(result.metadata_present);
    }

    #[test]
    fn full_source_and_output_destination_are_identity() {
        let result = classify(
            output_size(),
            output_size(),
            1,
            wl_output::Transform::Normal,
            Some(full_source()),
            Some(output_size()),
        )
        .unwrap();
        assert!(result.identity);
        assert!(result.metadata_present);
    }

    #[test]
    fn source_x_offset_is_non_identity() {
        let mut source = full_source();
        source.x = 1.0 / 256.0;
        assert_eq!(
            classify(
                output_size(),
                output_size(),
                1,
                wl_output::Transform::Normal,
                Some(source),
                None,
            ),
            Err(DirectScanoutSceneRejection::ViewportSourceNonIdentity)
        );
    }

    #[test]
    fn source_y_offset_is_non_identity() {
        let mut source = full_source();
        source.y = 1.0 / 256.0;
        assert_eq!(
            classify(
                output_size(),
                output_size(),
                1,
                wl_output::Transform::Normal,
                Some(source),
                None,
            ),
            Err(DirectScanoutSceneRejection::ViewportSourceNonIdentity)
        );
    }

    #[test]
    fn smaller_source_width_is_non_identity() {
        let mut source = full_source();
        source.width -= 1.0;
        assert_eq!(
            classify(
                output_size(),
                output_size(),
                1,
                wl_output::Transform::Normal,
                Some(source),
                None,
            ),
            Err(DirectScanoutSceneRejection::ViewportSourceNonIdentity)
        );
    }

    #[test]
    fn smaller_source_height_is_non_identity() {
        let mut source = full_source();
        source.height -= 1.0;
        assert_eq!(
            classify(
                output_size(),
                output_size(),
                1,
                wl_output::Transform::Normal,
                Some(source),
                None,
            ),
            Err(DirectScanoutSceneRejection::ViewportSourceNonIdentity)
        );
    }

    #[test]
    fn smaller_destination_is_non_identity() {
        assert_eq!(
            classify(
                output_size(),
                output_size(),
                1,
                wl_output::Transform::Normal,
                None,
                Some(BufferSize::new(1919, 1080).unwrap()),
            ),
            Err(DirectScanoutSceneRejection::ViewportDestinationNonIdentity)
        );
    }

    #[test]
    fn larger_destination_is_non_identity() {
        assert_eq!(
            classify(
                output_size(),
                output_size(),
                1,
                wl_output::Transform::Normal,
                None,
                Some(BufferSize::new(1921, 1080).unwrap()),
            ),
            Err(DirectScanoutSceneRejection::ViewportDestinationNonIdentity)
        );
    }

    #[test]
    fn buffer_size_mismatch_has_distinct_rejection() {
        assert_eq!(
            classify(
                BufferSize::new(1280, 720).unwrap(),
                output_size(),
                1,
                wl_output::Transform::Normal,
                None,
                None,
            ),
            Err(DirectScanoutSceneRejection::BufferSizeMismatch)
        );
    }

    #[test]
    fn non_unit_scale_has_distinct_rejection() {
        assert_eq!(
            classify(
                output_size(),
                output_size(),
                2,
                wl_output::Transform::Normal,
                None,
                None,
            ),
            Err(DirectScanoutSceneRejection::BufferScaleUnsupported)
        );
    }

    #[test]
    fn transformed_buffer_has_distinct_rejection() {
        assert_eq!(
            classify(
                output_size(),
                output_size(),
                1,
                wl_output::Transform::Flipped,
                None,
                None,
            ),
            Err(DirectScanoutSceneRejection::BufferTransformUnsupported)
        );
    }

    #[test]
    fn non_finite_source_is_non_identity() {
        let mut source = full_source();
        source.width = f64::NAN;
        assert_eq!(
            classify(
                output_size(),
                output_size(),
                1,
                wl_output::Transform::Normal,
                Some(source),
                None,
            ),
            Err(DirectScanoutSceneRejection::ViewportSourceNonIdentity)
        );
    }

    #[test]
    fn viewport_rejection_names_are_stable() {
        assert_eq!(
            DirectScanoutSceneRejection::ViewportSourceNonIdentity.as_str(),
            "viewport_source_non_identity"
        );
        assert_eq!(
            DirectScanoutSceneRejection::ViewportDestinationNonIdentity.as_str(),
            "viewport_destination_non_identity"
        );
    }
}
