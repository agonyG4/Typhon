use crate::core::{OutputId, SceneNodeId};

use super::{
    AnimationTime, PresentationGeometryTransform, PresentationRect, PresentationRevisionId,
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
    pub active_transitions: usize,
    pub sampled_windows: usize,
}

pub type FramePresentationSample = PresentationSceneSample;

impl PresentationSceneSample {
    pub fn empty(sampled_at: AnimationTime) -> Self {
        Self::empty_for_output(
            OutputId::from_raw(1).expect("single native output identity is nonzero"),
            sampled_at,
            PresentationSampleTimeSource::ZeroFallback,
        )
    }

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
}

#[derive(Debug, Clone, PartialEq)]
pub struct PresentationFrameSnapshot {
    pub output_id: OutputId,
    pub sampled_at: AnimationTime,
    pub sample_time_source: PresentationSampleTimeSource,
    pub transforms: Vec<PresentationGroupTransform>,
    pub presented_windows: Vec<PresentedWindowGeometry>,
    pub signature: u64,
}

impl PresentationFrameSnapshot {
    pub fn empty() -> Self {
        Self::from_sample(&PresentationSceneSample::empty(AnimationTime::from_nanos(
            0,
        )))
    }

    pub fn from_sample(sample: &PresentationSceneSample) -> Self {
        Self::from_sample_with_presented_windows(sample, Vec::new())
    }

    pub fn from_sample_with_presented_windows(
        sample: &PresentationSceneSample,
        mut presented_windows: Vec<PresentedWindowGeometry>,
    ) -> Self {
        presented_windows.sort_unstable_by_key(|window| window.root_surface_id());
        let mut snapshot = Self {
            output_id: sample.output_id,
            sampled_at: sample.sampled_at,
            sample_time_source: sample.sample_time_source,
            transforms: sample.transforms.clone(),
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

impl Default for PresentationFrameSnapshot {
    fn default() -> Self {
        Self::empty()
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
