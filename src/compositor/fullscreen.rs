#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FullscreenCompositionMode {
    #[default]
    Inactive,
    Transitioning,
    Dominant,
}

impl FullscreenCompositionMode {
    pub const fn is_dominant(self) -> bool {
        matches!(self, Self::Dominant)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullscreenAboveFullscreenReason {
    SpecialWorkspaceApplication,
    ApplicationNotification,
    ApplicationOverlay,
    ApplicationAbove,
    LayerOverlay,
}

impl FullscreenAboveFullscreenReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SpecialWorkspaceApplication => "special_workspace_application",
            Self::ApplicationNotification => "application_notification",
            Self::ApplicationOverlay => "application_overlay",
            Self::ApplicationAbove => "application_above",
            Self::LayerOverlay => "layer_overlay",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullscreenCulledRootReason {
    RegularApplication,
    OrdinaryApplicationPopup,
    LayerBackground,
    LayerBottom,
    LayerTop,
    GlobalContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullscreenRootClassification {
    OwnerFamily,
    AllowedAboveFullscreen(FullscreenAboveFullscreenReason),
    CulledByFullscreen(FullscreenCulledRootReason),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FullscreenCompositionPlan {
    pub owner_root_surface_id: Option<u32>,
    pub mode: FullscreenCompositionMode,
    pub owner_family_roots: Vec<u32>,
    pub allowed_application_roots: Vec<u32>,
    pub allowed_layer_roots: Vec<u32>,
    pub culled_application_roots: usize,
    pub culled_layer_roots: usize,
    pub culled_surface_count: usize,
    pub solitary_owner_only: bool,
    pub above_fullscreen_reason: Option<FullscreenAboveFullscreenReason>,
}

impl FullscreenCompositionPlan {
    pub fn allows_presentation_root(&self, root_surface_id: u32) -> bool {
        !self.mode.is_dominant()
            || self.owner_family_roots.contains(&root_surface_id)
            || self.allowed_application_roots.contains(&root_surface_id)
            || self.allowed_layer_roots.contains(&root_surface_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FullscreenPresentationState {
    pub owner_root_surface_id: u32,
    pub output_width: u32,
    pub output_height: u32,
}

pub(in crate::compositor) fn fullscreen_trace_enabled() -> bool {
    std::env::var_os("OBLIVION_ONE_DEBUG_FULLSCREEN").is_some_and(|value| value != "0")
}

// Kept private to this module for the existing unit tests. The scene-level
// Direct Scanout authority lives in `direct_scanout`.
#[cfg(test)]
pub(crate) use super::direct_scanout::{
    DirectScanoutSceneBlockers, DirectScanoutSceneRejection, DirectScanoutViewportCompatibility,
    direct_scanout_scene_rejection_for_effects, direct_scanout_scene_rejection_for_flags,
    direct_scanout_viewport_compatibility,
};

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FullscreenRenderPlanMetrics {
    pub fullscreen_active: bool,
    pub owner_root_surface_id: Option<u32>,
    pub fullscreen_composition_active: bool,
    pub fullscreen_transition_pending: bool,
    pub solitary_tree_active: bool,
    pub culled_surface_count: usize,
    pub wallpaper_culled: bool,
    pub visible_overlay_count: usize,
    pub fullscreen_allowed_application_roots: usize,
    pub fullscreen_allowed_layer_roots: usize,
    pub fullscreen_culled_application_roots: usize,
    pub fullscreen_culled_layer_roots: usize,
    pub fullscreen_above_reason: Option<FullscreenAboveFullscreenReason>,
    pub rejection: Option<FullscreenPresentationRejection>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::state_data::ViewportSourceRect;
    use crate::render_backend::buffer::BufferSize;
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
    fn visible_effect_blocks_direct_scanout_with_stable_reason() {
        let summary = crate::compositor::EffectSceneSummary {
            visible_instance_count: 1,
            requires_composition: true,
            continuous_instance_count: 0,
            maximum_capture_pixels: 80 * 40,
        };
        assert_eq!(
            direct_scanout_scene_rejection_for_effects(summary),
            Some(DirectScanoutSceneRejection::EffectRequiresComposition)
        );
    }

    #[test]
    fn removing_last_effect_restores_direct_scanout_eligibility() {
        let empty = crate::compositor::EffectSceneSummary::default();
        assert_eq!(direct_scanout_scene_rejection_for_effects(empty), None);
    }

    #[test]
    fn direct_scanout_scene_blockers_collect_unique_reasons_in_order() {
        let mut blockers = DirectScanoutSceneBlockers::default();
        blockers.push(DirectScanoutSceneRejection::OverlayVisible);
        blockers.push(DirectScanoutSceneRejection::PopupVisible);
        blockers.push(DirectScanoutSceneRejection::ResizePreviewActive);
        blockers.push(DirectScanoutSceneRejection::OverlayVisible);

        assert_eq!(
            blockers.reasons(),
            &[
                DirectScanoutSceneRejection::OverlayVisible,
                DirectScanoutSceneRejection::PopupVisible,
                DirectScanoutSceneRejection::ResizePreviewActive,
            ]
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

    #[test]
    fn animation_transform_rejection_has_stable_diagnostic_name() {
        assert_eq!(
            DirectScanoutSceneRejection::AnimationTransform.as_str(),
            "animation_transform"
        );
        assert_eq!(
            DirectScanoutSceneRejection::LifecycleAnimation.as_str(),
            "lifecycle_animation"
        );
    }
}
