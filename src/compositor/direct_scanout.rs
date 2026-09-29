use super::effects::{EffectAnchor, EffectAnchorScope};
use super::state_data::ViewportSourceRect;
use super::{BufferIdentity, BufferSize, DmabufBufferHandle, SurfaceCommitSequence};
use crate::core::SceneNodeId;
use crate::effects::{EffectInstanceId, EffectProgramId, EffectRegion};
use wayland_server::protocol::wl_output;

pub(crate) const MAX_DIRECT_SCANOUT_EFFECT_DETAILS: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectScanoutEffectDisposition {
    PresentationCulled,
    OutsideOutput,
    OccludedByOpaqueScanoutSource,
    ContributingAboveSource,
    ContributingAtSource,
    OutputPostProcess,
    UnknownOrder,
}

impl DirectScanoutEffectDisposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PresentationCulled => "presentation_culled",
            Self::OutsideOutput => "outside_output",
            Self::OccludedByOpaqueScanoutSource => "occluded_by_scanout_source",
            Self::ContributingAboveSource => "contributing_above_source",
            Self::ContributingAtSource => "contributing_at_source",
            Self::OutputPostProcess => "output_post_process",
            Self::UnknownOrder => "unknown_order",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectScanoutEffectInstanceAnalysis {
    pub id: EffectInstanceId,
    pub program: EffectProgramId,
    pub anchor: EffectAnchor,
    pub region: EffectRegion,
    pub disposition: DirectScanoutEffectDisposition,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirectScanoutEffectAnalysis {
    pub raw_instance_count: u32,
    pub presentation_instance_count: u32,
    pub culled_instance_count: u32,
    pub outside_output_instance_count: u32,
    pub occluded_instance_count: u32,
    pub contributing_instance_count: u32,
    pub requires_composition: bool,
    pub instances: Vec<DirectScanoutEffectInstanceAnalysis>,
    pub instances_truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirectScanoutEffectDoctorDetails {
    pub raw_instance_count: u32,
    pub presentation_instance_count: u32,
    pub culled_instance_count: u32,
    pub outside_output_instance_count: u32,
    pub occluded_instance_count: u32,
    pub contributing_instance_count: u32,
    pub requires_composition: bool,
    pub details: Vec<String>,
    pub details_truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DirectScanoutEffectSource {
    pub(crate) group_order: Option<u32>,
    pub(crate) surface_order: Option<u32>,
    pub(crate) can_occlude: bool,
}

#[derive(Debug, Clone)]
pub struct DirectScanoutSceneCandidate {
    pub surface_id: u32,
    pub root_surface_id: u32,
    pub surface_scene_node_id: SceneNodeId,
    pub window_scene_node_id: SceneNodeId,
    pub presented_window_rect: crate::compositor::PresentationRect,
    pub render_generation: u64,
    pub effect_identity_signature: u64,
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

/// A scene that passes every direct-scanout scene check except for the
/// source-buffer/output-mode size mismatch. This is diagnostic metadata only;
/// it must never be used to plan presentation.
#[derive(Debug, Clone)]
pub struct DirectScanoutProbeCandidate {
    pub buffer_identity: BufferIdentity,
    pub buffer: DmabufBufferHandle,
    pub buffer_size: BufferSize,
    pub output_size: BufferSize,
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
    PresentationClip,
    PlacementMismatch,
    ResizePreviewActive,
    AnimationTransform,
    PresentationOpacity,
    LifecycleAnimation,
    WindowExitAnimation,
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
            Self::PresentationClip => "presentation_clip",
            Self::PlacementMismatch => "placement_mismatch",
            Self::ResizePreviewActive => "resize_preview_active",
            Self::AnimationTransform => "animation_transform",
            Self::PresentationOpacity => "presentation_opacity",
            Self::LifecycleAnimation => "lifecycle_animation",
            Self::WindowExitAnimation => "window_exit_animation",
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

#[cfg(test)]
pub(crate) fn classify_direct_scanout_effect(
    instance: &crate::compositor::ResolvedEffectInstance,
    source: DirectScanoutEffectSource,
    anchor_surface_order: Option<u32>,
) -> DirectScanoutEffectDisposition {
    classify_direct_scanout_effect_with_scene_orders(
        instance,
        source,
        instance.visual_group.map(|group| group.get()),
        anchor_surface_order,
    )
}

pub(crate) fn classify_direct_scanout_effect_with_scene_orders(
    instance: &crate::compositor::ResolvedEffectInstance,
    source: DirectScanoutEffectSource,
    effect_group_order: Option<u32>,
    anchor_surface_order: Option<u32>,
) -> DirectScanoutEffectDisposition {
    if instance.anchor == EffectAnchor::OutputPostProcess {
        return DirectScanoutEffectDisposition::OutputPostProcess;
    }
    let Some(effect_group_order) = effect_group_order else {
        return DirectScanoutEffectDisposition::UnknownOrder;
    };
    let Some(source_group_order) = source.group_order else {
        return DirectScanoutEffectDisposition::UnknownOrder;
    };

    if effect_group_order < source_group_order {
        return if source.can_occlude {
            DirectScanoutEffectDisposition::OccludedByOpaqueScanoutSource
        } else {
            DirectScanoutEffectDisposition::UnknownOrder
        };
    }
    if effect_group_order > source_group_order {
        return DirectScanoutEffectDisposition::ContributingAboveSource;
    }

    if instance.anchor_scope == EffectAnchorScope::VisualGroup {
        return match instance.anchor {
            EffectAnchor::BeforeSurface(_) if source.can_occlude => {
                DirectScanoutEffectDisposition::OccludedByOpaqueScanoutSource
            }
            EffectAnchor::BeforeSurface(_) => DirectScanoutEffectDisposition::UnknownOrder,
            EffectAnchor::ReplaceSurface(_) | EffectAnchor::AfterSurface(_) => {
                DirectScanoutEffectDisposition::ContributingAtSource
            }
            EffectAnchor::OutputPostProcess => DirectScanoutEffectDisposition::OutputPostProcess,
        };
    }

    let Some(source_surface_order) = source.surface_order else {
        return DirectScanoutEffectDisposition::UnknownOrder;
    };
    let Some(anchor_surface_order) = anchor_surface_order else {
        return DirectScanoutEffectDisposition::UnknownOrder;
    };
    if anchor_surface_order < source_surface_order {
        return if source.can_occlude {
            DirectScanoutEffectDisposition::OccludedByOpaqueScanoutSource
        } else {
            DirectScanoutEffectDisposition::UnknownOrder
        };
    }
    if anchor_surface_order > source_surface_order {
        return DirectScanoutEffectDisposition::ContributingAboveSource;
    }
    match instance.anchor {
        EffectAnchor::BeforeSurface(_) if source.can_occlude => {
            DirectScanoutEffectDisposition::OccludedByOpaqueScanoutSource
        }
        EffectAnchor::BeforeSurface(_) => DirectScanoutEffectDisposition::UnknownOrder,
        EffectAnchor::ReplaceSurface(_) | EffectAnchor::AfterSurface(_) => {
            DirectScanoutEffectDisposition::ContributingAtSource
        }
        EffectAnchor::OutputPostProcess => DirectScanoutEffectDisposition::OutputPostProcess,
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
    direct_scanout_probe_viewport_compatibility(
        buffer_size,
        output_size,
        buffer_scale,
        buffer_transform,
        viewport_source,
        viewport_destination,
    )
}

pub(crate) fn direct_scanout_probe_viewport_compatibility(
    buffer_size: BufferSize,
    output_size: BufferSize,
    buffer_scale: u32,
    buffer_transform: wl_output::Transform,
    viewport_source: Option<ViewportSourceRect>,
    viewport_destination: Option<BufferSize>,
) -> Result<DirectScanoutViewportCompatibility, DirectScanoutSceneRejection> {
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

pub(crate) fn direct_scanout_probe_blockers_allow_scaling(
    blockers: &DirectScanoutSceneBlockers,
) -> bool {
    blockers.reasons() == [DirectScanoutSceneRejection::BufferSizeMismatch]
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

#[cfg(test)]
pub(crate) const fn direct_scanout_scene_rejection_for_effects(
    summary: crate::compositor::EffectSceneSummary,
) -> Option<DirectScanoutSceneRejection> {
    if summary.requires_composition && summary.visible_instance_count > 0 {
        Some(DirectScanoutSceneRejection::EffectRequiresComposition)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::{EffectAnchorScope, EffectSceneOrder, VisualGroupId};
    use crate::effects::{
        EffectFrameDemand, EffectInstanceId, EffectParameterBlock, EffectProgramId, EffectRect,
        EffectRegion,
    };

    fn effect(
        anchor: EffectAnchor,
        anchor_scope: EffectAnchorScope,
        group: u32,
    ) -> crate::compositor::ResolvedEffectInstance {
        let region = EffectRegion::from_rect(EffectRect::new(0, 0, 10, 10).unwrap());
        crate::compositor::ResolvedEffectInstance {
            id: EffectInstanceId::new(1).unwrap(),
            program: EffectProgramId::new(2).unwrap(),
            anchor,
            region: region.clone(),
            target_bounds: EffectRect::new(0, 0, 10, 10).unwrap(),
            parameter_block: EffectParameterBlock::default(),
            signature: 0,
            frame_demand: EffectFrameDemand::OnDamage,
            visual_group: VisualGroupId::new(group),
            anchor_scope,
            scene_order: EffectSceneOrder::for_anchor(anchor),
        }
    }

    fn source() -> DirectScanoutEffectSource {
        DirectScanoutEffectSource {
            group_order: Some(2),
            surface_order: Some(1),
            can_occlude: true,
        }
    }

    #[test]
    fn scaled_probe_viewport_accepts_only_full_source_full_output_identity_state() {
        let source = BufferSize::new(1600, 900).unwrap();
        let output = BufferSize::new(1920, 1080).unwrap();
        assert!(
            direct_scanout_probe_viewport_compatibility(
                source,
                output,
                1,
                wl_output::Transform::Normal,
                Some(ViewportSourceRect {
                    x: 0.0,
                    y: 0.0,
                    width: 1600.0,
                    height: 900.0,
                }),
                Some(output),
            )
            .is_ok()
        );

        assert_eq!(
            direct_scanout_probe_viewport_compatibility(
                source,
                output,
                2,
                wl_output::Transform::Normal,
                None,
                None,
            ),
            Err(DirectScanoutSceneRejection::BufferScaleUnsupported)
        );
        assert_eq!(
            direct_scanout_probe_viewport_compatibility(
                source,
                output,
                1,
                wl_output::Transform::_90,
                None,
                None,
            ),
            Err(DirectScanoutSceneRejection::BufferTransformUnsupported)
        );
        assert_eq!(
            direct_scanout_probe_viewport_compatibility(
                source,
                output,
                1,
                wl_output::Transform::Normal,
                Some(ViewportSourceRect {
                    x: 1.0,
                    y: 0.0,
                    width: 1599.0,
                    height: 900.0,
                }),
                None,
            ),
            Err(DirectScanoutSceneRejection::ViewportSourceNonIdentity)
        );
        assert_eq!(
            direct_scanout_probe_viewport_compatibility(
                source,
                output,
                1,
                wl_output::Transform::Normal,
                None,
                Some(BufferSize::new(1800, 1000).unwrap()),
            ),
            Err(DirectScanoutSceneRejection::ViewportDestinationNonIdentity)
        );
    }

    #[test]
    fn only_buffer_size_mismatch_is_relaxed_for_the_probe() {
        let eligible = DirectScanoutSceneBlockers {
            reasons: vec![DirectScanoutSceneRejection::BufferSizeMismatch],
        };
        assert!(direct_scanout_probe_blockers_allow_scaling(&eligible));
        for blocker in [
            DirectScanoutSceneRejection::OverlayVisible,
            DirectScanoutSceneRejection::PopupVisible,
            DirectScanoutSceneRejection::EffectRequiresComposition,
            DirectScanoutSceneRejection::ServerSideDecorationVisible,
            DirectScanoutSceneRejection::AnimationTransform,
            DirectScanoutSceneRejection::LifecycleAnimation,
            DirectScanoutSceneRejection::WindowExitAnimation,
            DirectScanoutSceneRejection::PresentationOpacity,
            DirectScanoutSceneRejection::PresentationClip,
            DirectScanoutSceneRejection::VisualClipPresent,
            DirectScanoutSceneRejection::ResizePreviewActive,
        ] {
            let mut blocked = eligible.clone();
            blocked.push(blocker);
            assert!(!direct_scanout_probe_blockers_allow_scaling(&blocked));
        }
        assert!(!direct_scanout_probe_blockers_allow_scaling(
            &DirectScanoutSceneBlockers::default()
        ));
    }

    #[test]
    fn source_relative_effect_order_uses_scope_and_anchor_phase() {
        let source = source();
        assert_eq!(
            classify_direct_scanout_effect(
                &effect(
                    EffectAnchor::BeforeSurface(20),
                    EffectAnchorScope::Surface,
                    2,
                ),
                source,
                Some(1),
            ),
            DirectScanoutEffectDisposition::OccludedByOpaqueScanoutSource
        );
        assert_eq!(
            classify_direct_scanout_effect(
                &effect(
                    EffectAnchor::ReplaceSurface(20),
                    EffectAnchorScope::Surface,
                    2,
                ),
                source,
                Some(1),
            ),
            DirectScanoutEffectDisposition::ContributingAtSource
        );
        assert_eq!(
            classify_direct_scanout_effect(
                &effect(
                    EffectAnchor::AfterSurface(20),
                    EffectAnchorScope::Surface,
                    2
                ),
                source,
                Some(1),
            ),
            DirectScanoutEffectDisposition::ContributingAtSource
        );
        assert_eq!(
            classify_direct_scanout_effect(
                &effect(
                    EffectAnchor::BeforeSurface(10),
                    EffectAnchorScope::Surface,
                    2
                ),
                source,
                Some(0),
            ),
            DirectScanoutEffectDisposition::OccludedByOpaqueScanoutSource
        );
        assert_eq!(
            classify_direct_scanout_effect(
                &effect(
                    EffectAnchor::BeforeSurface(30),
                    EffectAnchorScope::Surface,
                    2
                ),
                source,
                Some(2),
            ),
            DirectScanoutEffectDisposition::ContributingAboveSource
        );
        assert_eq!(
            classify_direct_scanout_effect(
                &effect(
                    EffectAnchor::BeforeSurface(10),
                    EffectAnchorScope::VisualGroup,
                    2,
                ),
                source,
                None,
            ),
            DirectScanoutEffectDisposition::OccludedByOpaqueScanoutSource
        );
        assert_eq!(
            classify_direct_scanout_effect(
                &effect(
                    EffectAnchor::ReplaceSurface(10),
                    EffectAnchorScope::VisualGroup,
                    2,
                ),
                source,
                None,
            ),
            DirectScanoutEffectDisposition::ContributingAtSource
        );
        assert_eq!(
            classify_direct_scanout_effect(
                &effect(
                    EffectAnchor::AfterSurface(10),
                    EffectAnchorScope::VisualGroup,
                    1
                ),
                source,
                None,
            ),
            DirectScanoutEffectDisposition::OccludedByOpaqueScanoutSource
        );
        assert_eq!(
            classify_direct_scanout_effect(
                &effect(
                    EffectAnchor::AfterSurface(30),
                    EffectAnchorScope::Surface,
                    3
                ),
                source,
                Some(0),
            ),
            DirectScanoutEffectDisposition::ContributingAboveSource
        );
        assert_eq!(
            classify_direct_scanout_effect(
                &effect(
                    EffectAnchor::BeforeSurface(10),
                    EffectAnchorScope::Surface,
                    1
                ),
                DirectScanoutEffectSource {
                    group_order: None,
                    ..source
                },
                Some(0),
            ),
            DirectScanoutEffectDisposition::UnknownOrder
        );
    }
}
