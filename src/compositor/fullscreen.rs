#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FullscreenPresentationState {
    pub owner_root_surface_id: u32,
    pub output_width: u32,
    pub output_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullscreenPresentationRejection {
    NoFullscreenOwner,
    OwnerMissing,
    OwnerMinimized,
    OwnerDoesNotCoverOutput,
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
