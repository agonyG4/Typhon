use crate::core::{OutputId, SceneNodeId};

use super::{
    AnimationTime, PresentationClip, PresentationClipRect, PresentationGeometryTransform,
    PresentationOpacity, PresentationPropertyKind, PresentationRect, PresentationRevisionId,
    PresentationTransactionId, PresentationWindowSample, TransitionId,
};

/// The source used to choose the immutable timestamp attached to a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationSampleTimeSource {
    ScheduledTarget,
    MonotonicFallback,
    ZeroFallback,
}

/// Frame-local target: the WindowGroup owns presentation state while the
/// root surface remains the current render/input adapter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationWindowTarget {
    window_group_scene_node_id: SceneNodeId,
    root_surface_id: u32,
    canonical_rect: PresentationRect,
    canonical_opacity: PresentationOpacity,
    canonical_clip: PresentationClip,
}

impl PresentationWindowTarget {
    pub const fn with_scene_node(
        window_group_scene_node_id: SceneNodeId,
        root_surface_id: u32,
        canonical_rect: PresentationRect,
    ) -> Self {
        Self {
            window_group_scene_node_id,
            root_surface_id,
            canonical_rect,
            canonical_opacity: PresentationOpacity::OPAQUE,
            canonical_clip: PresentationClip::Unbounded,
        }
    }

    #[cfg(test)]
    pub const fn new(root_surface_id: u32, canonical_rect: PresentationRect) -> Self {
        Self::with_scene_node(
            synthetic_scene_node_id(root_surface_id),
            root_surface_id,
            canonical_rect,
        )
    }

    pub const fn window_group_scene_node_id(self) -> SceneNodeId {
        self.window_group_scene_node_id
    }

    pub const fn scene_node_id(self) -> SceneNodeId {
        self.window_group_scene_node_id()
    }

    pub const fn root_surface_id(self) -> u32 {
        self.root_surface_id
    }

    pub const fn canonical_rect(self) -> PresentationRect {
        self.canonical_rect
    }

    pub const fn with_canonical_opacity(mut self, canonical_opacity: PresentationOpacity) -> Self {
        self.canonical_opacity = canonical_opacity;
        self
    }

    pub const fn canonical_opacity(self) -> PresentationOpacity {
        self.canonical_opacity
    }

    pub const fn with_canonical_clip(mut self, canonical_clip: PresentationClip) -> Self {
        self.canonical_clip = canonical_clip;
        self
    }

    pub const fn canonical_clip(self) -> PresentationClip {
        self.canonical_clip
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NativeFramePresentationTargets {
    windows: Vec<PresentationWindowTarget>,
}

impl NativeFramePresentationTargets {
    pub(crate) fn from_windows(windows: Vec<PresentationWindowTarget>) -> Self {
        Self { windows }
    }

    pub fn windows(&self) -> &[PresentationWindowTarget] {
        &self.windows
    }

    pub fn root_surface_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.windows.iter().map(|window| window.root_surface_id())
    }

    pub fn scene_node_ids(&self) -> impl Iterator<Item = SceneNodeId> + '_ {
        self.windows
            .iter()
            .map(|window| window.window_group_scene_node_id())
    }
}

/// Immutable evidence frozen into one frame snapshot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationGroupTransform {
    pub scene_node_id: SceneNodeId,
    pub root_surface_id: u32,
    pub transaction_id: PresentationTransactionId,
    pub revision_id: PresentationRevisionId,
    /// Compatibility field; equal to `revision_id` during the migration.
    pub transition_id: TransitionId,
    pub canonical_rect: PresentationRect,
    pub presented_rect: PresentationRect,
    pub mathematically_settled: bool,
}

impl PresentationGroupTransform {
    pub const fn with_scene_node(
        scene_node_id: SceneNodeId,
        root_surface_id: u32,
        transaction_id: PresentationTransactionId,
        revision_id: PresentationRevisionId,
        canonical_rect: PresentationRect,
        presented_rect: PresentationRect,
        mathematically_settled: bool,
    ) -> Self {
        Self {
            scene_node_id,
            root_surface_id,
            transaction_id,
            revision_id,
            transition_id: revision_id,
            canonical_rect,
            presented_rect,
            mathematically_settled,
        }
    }

    #[cfg(test)]
    pub const fn new(
        root_surface_id: u32,
        transition_id: TransitionId,
        canonical_rect: PresentationRect,
        presented_rect: PresentationRect,
        mathematically_settled: bool,
    ) -> Self {
        Self::with_scene_node(
            synthetic_scene_node_id(root_surface_id),
            root_surface_id,
            PresentationTransactionId::new(std::num::NonZeroU64::MIN),
            transition_id,
            canonical_rect,
            presented_rect,
            mathematically_settled,
        )
    }

    pub fn map_point(self, point: (f64, f64)) -> (f64, f64) {
        self.geometry().map_point(point)
    }

    pub fn inverse_map_point(self, point: (f64, f64)) -> Option<(f64, f64)> {
        self.inverse_map_point_unbounded(point)
    }

    pub fn inverse_map_point_unbounded(self, point: (f64, f64)) -> Option<(f64, f64)> {
        self.geometry().inverse_map_point_unbounded(point)
    }

    pub fn map_rect(self, rect: PresentationRect) -> Option<PresentationRect> {
        self.geometry().map_rect(rect)
    }

    pub fn map_clip_rect(self, rect: PresentationClipRect) -> Option<PresentationClipRect> {
        let local_top_left = (
            self.canonical_rect.x() + rect.x(),
            self.canonical_rect.y() + rect.y(),
        );
        let local_bottom_right = (
            local_top_left.0 + rect.width(),
            local_top_left.1 + rect.height(),
        );
        let top_left = self.map_point(local_top_left);
        let bottom_right = self.map_point(local_bottom_right);
        PresentationClipRect::new(
            top_left.0,
            top_left.1,
            bottom_right.0 - top_left.0,
            bottom_right.1 - top_left.1,
        )
    }

    pub fn map_rect_outward(self, rect: PresentationRect) -> Option<super::PresentationDamageRect> {
        self.map_rect(rect)
            .map(|mapped| super::presentation_damage(mapped, mapped))
    }

    pub fn scale_x(self) -> f64 {
        self.geometry().scale_x()
    }

    pub fn scale_y(self) -> f64 {
        self.geometry().scale_y()
    }

    pub fn is_identity(self) -> bool {
        self.geometry().is_identity()
    }

    pub const fn geometry(self) -> PresentationGeometryTransform {
        PresentationGeometryTransform::new(self.canonical_rect, self.presented_rect)
    }

    /// The visual signature intentionally retains root and revision identity,
    /// but does not invalidate pixel caches for SceneNode or transaction IDs.
    pub fn signature(self) -> u64 {
        let mut signature = 0xcbf2_9ce4_8422_2325_u64;
        for value in [
            u64::from(self.root_surface_id),
            self.revision_id.get(),
            self.canonical_rect.x().to_bits(),
            self.canonical_rect.y().to_bits(),
            self.canonical_rect.width().to_bits(),
            self.canonical_rect.height().to_bits(),
            self.presented_rect.x().to_bits(),
            self.presented_rect.y().to_bits(),
            self.presented_rect.width().to_bits(),
            self.presented_rect.height().to_bits(),
            self.mathematically_settled as u64,
        ] {
            signature ^= value;
            signature = signature.wrapping_mul(0x1000_0000_01b3);
        }
        signature
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationOpacityTransitionEvidence {
    pub transaction_id: PresentationTransactionId,
    pub revision_id: PresentationRevisionId,
    pub mathematically_settled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationGroupOpacity {
    pub scene_node_id: SceneNodeId,
    pub root_surface_id: u32,
    pub opacity: PresentationOpacity,
    pub transition: Option<PresentationOpacityTransitionEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationClipTransitionEvidence {
    pub transaction_id: PresentationTransactionId,
    pub revision_id: PresentationRevisionId,
    pub mathematically_settled: bool,
}

/// Immutable WindowGroup-local Clip and its exact output-space projection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationGroupClip {
    pub scene_node_id: SceneNodeId,
    pub root_surface_id: u32,
    pub clip: PresentationClip,
    pub presented_clip: Option<PresentationClipRect>,
    pub transition: Option<PresentationClipTransitionEvidence>,
}

impl PresentationGroupClip {
    pub const fn with_scene_node(
        scene_node_id: SceneNodeId,
        root_surface_id: u32,
        clip: PresentationClip,
        presented_clip: Option<PresentationClipRect>,
        transition: Option<PresentationClipTransitionEvidence>,
    ) -> Self {
        Self {
            scene_node_id,
            root_surface_id,
            clip,
            presented_clip,
            transition,
        }
    }

    /// Identity clips are deliberately omitted from pixel identity. Their
    /// exact transition evidence remains available for physical ACK.
    pub fn visual_signature(self) -> Option<u64> {
        let clip = self.presented_clip?;
        let mut signature = 0xcbf2_9ce4_8422_2325_u64;
        for value in [
            u64::from(self.root_surface_id),
            clip.x().to_bits(),
            clip.y().to_bits(),
            clip.width().to_bits(),
            clip.height().to_bits(),
        ] {
            signature ^= value;
            signature = signature.wrapping_mul(0x1000_0000_01b3);
        }
        Some(signature)
    }
}

impl PresentationGroupOpacity {
    pub const fn with_scene_node(
        scene_node_id: SceneNodeId,
        root_surface_id: u32,
        opacity: PresentationOpacity,
        transition: Option<PresentationOpacityTransitionEvidence>,
    ) -> Self {
        Self {
            scene_node_id,
            root_surface_id,
            opacity,
            transition,
        }
    }

    pub fn signature(self) -> u64 {
        let mut signature = 0xcbf2_9ce4_8422_2325_u64;
        for value in [
            u64::from(self.root_surface_id),
            self.opacity.get().to_bits(),
        ] {
            signature ^= value;
            signature = signature.wrapping_mul(0x1000_0000_01b3);
        }
        signature
    }
}

/// Immutable physical evidence used to acknowledge one settled geometry track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentedGeometryAck {
    pub output_id: OutputId,
    pub scene_node_id: SceneNodeId,
    pub property: PresentationPropertyKind,
    pub transaction_id: PresentationTransactionId,
    pub revision_id: PresentationRevisionId,
    pub presented_rect: PresentationRect,
}

impl PresentedGeometryAck {
    pub const fn from_transform(
        output_id: OutputId,
        transform: PresentationGroupTransform,
    ) -> Self {
        Self {
            output_id,
            scene_node_id: transform.scene_node_id,
            property: PresentationPropertyKind::Geometry,
            transaction_id: transform.transaction_id,
            revision_id: transform.revision_id,
            presented_rect: transform.presented_rect,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentedOpacityAck {
    pub output_id: OutputId,
    pub scene_node_id: SceneNodeId,
    pub property: PresentationPropertyKind,
    pub transaction_id: PresentationTransactionId,
    pub revision_id: PresentationRevisionId,
    pub presented_opacity: PresentationOpacity,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentedClipAck {
    pub output_id: OutputId,
    pub scene_node_id: SceneNodeId,
    pub property: PresentationPropertyKind,
    pub transaction_id: PresentationTransactionId,
    pub revision_id: PresentationRevisionId,
    pub presented_clip: PresentationClip,
}

impl PresentedClipAck {
    pub const fn from_group_clip(
        output_id: OutputId,
        group: PresentationGroupClip,
    ) -> Option<Self> {
        let Some(transition) = group.transition else {
            return None;
        };
        Some(Self {
            output_id,
            scene_node_id: group.scene_node_id,
            property: PresentationPropertyKind::Clip,
            transaction_id: transition.transaction_id,
            revision_id: transition.revision_id,
            presented_clip: group.clip,
        })
    }
}

impl PresentedOpacityAck {
    pub const fn from_group_opacity(
        output_id: OutputId,
        group: PresentationGroupOpacity,
    ) -> Option<Self> {
        let Some(transition) = group.transition else {
            return None;
        };
        Some(Self {
            output_id,
            scene_node_id: group.scene_node_id,
            property: PresentationPropertyKind::Opacity,
            transaction_id: transition.transaction_id,
            revision_id: transition.revision_id,
            presented_opacity: group.opacity,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentedWindowGeometry {
    scene_node_id: SceneNodeId,
    root_surface_id: u32,
    presented_rect: PresentationRect,
}

impl PresentedWindowGeometry {
    pub const fn with_scene_node(
        scene_node_id: SceneNodeId,
        root_surface_id: u32,
        presented_rect: PresentationRect,
    ) -> Self {
        Self {
            scene_node_id,
            root_surface_id,
            presented_rect,
        }
    }

    #[cfg(test)]
    pub const fn new(root_surface_id: u32, presented_rect: PresentationRect) -> Self {
        Self::with_scene_node(
            synthetic_scene_node_id(root_surface_id),
            root_surface_id,
            presented_rect,
        )
    }

    pub const fn scene_node_id(&self) -> SceneNodeId {
        self.scene_node_id
    }

    pub const fn root_surface_id(&self) -> u32 {
        self.root_surface_id
    }

    pub const fn presented_rect(self) -> PresentationRect {
        self.presented_rect
    }
}

/// The v2 frame sample. It is retained as `PresentationSceneSample` for
/// source compatibility with existing compositor paths.
#[derive(Debug, Clone, PartialEq)]
pub struct PresentationSceneSample {
    pub output_id: OutputId,
    pub sampled_at: AnimationTime,
    pub sample_time_source: PresentationSampleTimeSource,
    pub windows: Vec<PresentationWindowSample>,
    pub transforms: Vec<PresentationGroupTransform>,
    pub opacities: Vec<PresentationGroupOpacity>,
    pub clips: Vec<PresentationGroupClip>,
    pub active_transitions: usize,
    pub sampled_windows: usize,
}

pub type FramePresentationSample = PresentationSceneSample;

impl PresentationSceneSample {
    pub fn empty_for_output(
        output_id: OutputId,
        sampled_at: AnimationTime,
        sample_time_source: PresentationSampleTimeSource,
    ) -> Self {
        Self {
            output_id,
            sampled_at,
            sample_time_source,
            windows: Vec::new(),
            transforms: Vec::new(),
            opacities: Vec::new(),
            clips: Vec::new(),
            active_transitions: 0,
            sampled_windows: 0,
        }
    }

    pub fn transform_for_root(&self, root_surface_id: u32) -> Option<PresentationGroupTransform> {
        self.transforms
            .iter()
            .find(|transform| transform.root_surface_id == root_surface_id)
            .copied()
    }

    pub fn transform_for_scene_node(
        &self,
        scene_node_id: SceneNodeId,
    ) -> Option<PresentationGroupTransform> {
        self.transforms
            .iter()
            .find(|transform| transform.scene_node_id == scene_node_id)
            .copied()
    }

    pub fn frame_snapshot(&self) -> PresentationFrameSnapshot {
        PresentationFrameSnapshot::from_sample(self)
    }

    pub fn geometry_signature(&self) -> u64 {
        self.frame_snapshot().signature
    }

    pub fn presentation_visual_signature(&self) -> u64 {
        self.frame_snapshot().signature
    }

    pub fn opacity_for_root(&self, root_surface_id: u32) -> PresentationOpacity {
        self.opacities
            .binary_search_by_key(&root_surface_id, |opacity| opacity.root_surface_id)
            .ok()
            .map(|index| self.opacities[index].opacity)
            .unwrap_or(PresentationOpacity::OPAQUE)
    }

    pub fn opacity_for_scene_node(&self, scene_node_id: SceneNodeId) -> PresentationOpacity {
        self.opacities
            .iter()
            .find(|opacity| opacity.scene_node_id == scene_node_id)
            .map_or(PresentationOpacity::OPAQUE, |opacity| opacity.opacity)
    }

    pub fn clip_for_root(&self, root_surface_id: u32) -> PresentationClip {
        self.clips
            .binary_search_by_key(&root_surface_id, |clip| clip.root_surface_id)
            .ok()
            .map(|index| self.clips[index].clip)
            .unwrap_or(PresentationClip::Unbounded)
    }

    pub fn clip_for_scene_node(&self, scene_node_id: SceneNodeId) -> PresentationClip {
        self.clips
            .iter()
            .find(|clip| clip.scene_node_id == scene_node_id)
            .map_or(PresentationClip::Unbounded, |clip| clip.clip)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PresentationFrameSnapshot {
    pub output_id: OutputId,
    pub sampled_at: AnimationTime,
    pub sample_time_source: PresentationSampleTimeSource,
    pub transforms: Vec<PresentationGroupTransform>,
    pub opacities: Vec<PresentationGroupOpacity>,
    pub clips: Vec<PresentationGroupClip>,
    pub presented_windows: Vec<PresentedWindowGeometry>,
    pub signature: u64,
}

impl PresentationFrameSnapshot {
    pub fn empty_for_output(output_id: OutputId) -> Self {
        Self::from_sample(&PresentationSceneSample::empty_for_output(
            output_id,
            AnimationTime::from_nanos(0),
            PresentationSampleTimeSource::ZeroFallback,
        ))
    }

    pub fn from_sample(sample: &PresentationSceneSample) -> Self {
        Self::from_sample_with_presented_windows(sample, Vec::new())
    }

    pub fn from_sample_with_presented_windows(
        sample: &PresentationSceneSample,
        mut presented_windows: Vec<PresentedWindowGeometry>,
    ) -> Self {
        presented_windows.sort_unstable_by_key(|window| window.root_surface_id());
        let mut opacities = sample.opacities.clone();
        opacities.sort_unstable_by_key(|opacity| opacity.root_surface_id);
        let mut snapshot = Self {
            output_id: sample.output_id,
            sampled_at: sample.sampled_at,
            sample_time_source: sample.sample_time_source,
            transforms: sample.transforms.clone(),
            opacities,
            clips: {
                let mut clips = sample.clips.clone();
                clips.sort_unstable_by_key(|clip| clip.root_surface_id);
                clips
            },
            presented_windows,
            signature: 0,
        };
        snapshot.refresh_signature();
        snapshot
    }

    pub fn transform_for_root(&self, root_surface_id: u32) -> Option<PresentationGroupTransform> {
        self.transforms
            .iter()
            .find(|transform| transform.root_surface_id == root_surface_id)
            .copied()
    }

    pub fn transform_for_scene_node(
        &self,
        scene_node_id: SceneNodeId,
    ) -> Option<PresentationGroupTransform> {
        self.transforms
            .iter()
            .find(|transform| transform.scene_node_id == scene_node_id)
            .copied()
    }

    pub fn opacity_for_root(&self, root_surface_id: u32) -> PresentationOpacity {
        self.opacities
            .binary_search_by_key(&root_surface_id, |opacity| opacity.root_surface_id)
            .ok()
            .map(|index| self.opacities[index].opacity)
            .unwrap_or(PresentationOpacity::OPAQUE)
    }

    pub fn opacity_for_scene_node(&self, scene_node_id: SceneNodeId) -> PresentationOpacity {
        self.opacities
            .iter()
            .find(|opacity| opacity.scene_node_id == scene_node_id)
            .map_or(PresentationOpacity::OPAQUE, |opacity| opacity.opacity)
    }

    pub fn clip_for_root(&self, root_surface_id: u32) -> PresentationClip {
        self.clips
            .binary_search_by_key(&root_surface_id, |clip| clip.root_surface_id)
            .ok()
            .map(|index| self.clips[index].clip)
            .unwrap_or(PresentationClip::Unbounded)
    }

    pub fn clip_for_scene_node(&self, scene_node_id: SceneNodeId) -> PresentationClip {
        self.clips
            .iter()
            .find(|clip| clip.scene_node_id == scene_node_id)
            .map_or(PresentationClip::Unbounded, |clip| clip.clip)
    }

    pub fn presented_window_geometry(
        &self,
        root_surface_id: u32,
    ) -> Option<PresentedWindowGeometry> {
        self.presented_windows
            .binary_search_by_key(&root_surface_id, PresentedWindowGeometry::root_surface_id)
            .ok()
            .map(|index| self.presented_windows[index])
    }

    pub fn presented_window_geometry_for_scene_node(
        &self,
        scene_node_id: SceneNodeId,
    ) -> Option<PresentedWindowGeometry> {
        self.presented_windows
            .iter()
            .find(|window| window.scene_node_id == scene_node_id)
            .copied()
    }

    pub fn is_identity_for_root(&self, root_surface_id: u32) -> bool {
        self.transform_for_root(root_surface_id)
            .is_none_or(PresentationGroupTransform::is_identity)
    }

    pub fn refresh_signature(&mut self) {
        let mut signature = 0xcbf2_9ce4_8422_2325_u64;
        for transform in &self.transforms {
            signature ^= transform.signature();
            signature = signature.wrapping_mul(0x1000_0000_01b3);
        }
        for opacity in &self.opacities {
            signature ^= opacity.signature();
            signature = signature.wrapping_mul(0x1000_0000_01b3);
        }
        for clip in &self.clips {
            if let Some(clip_signature) = clip.visual_signature() {
                signature ^= clip_signature;
                signature = signature.wrapping_mul(0x1000_0000_01b3);
            }
        }
        for window in &self.presented_windows {
            signature ^= u64::from(window.root_surface_id());
            signature = signature.wrapping_mul(0x1000_0000_01b3);
            let rect = window.presented_rect();
            for value in [
                rect.x().to_bits(),
                rect.y().to_bits(),
                rect.width().to_bits(),
                rect.height().to_bits(),
            ] {
                signature ^= value;
                signature = signature.wrapping_mul(0x1000_0000_01b3);
            }
        }
        self.signature = signature;
    }
}

#[cfg(test)]
const fn synthetic_scene_node_id(root_surface_id: u32) -> SceneNodeId {
    SceneNodeId::from_raw(if root_surface_id == 0 {
        1
    } else {
        root_surface_id as u64
    })
    .expect("synthetic test scene node id is nonzero")
}
