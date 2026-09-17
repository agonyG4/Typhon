use std::borrow::Cow;

use super::super::NativeSceneSnapshot;
use oblivion_one::compositor::{
    FullscreenPresentationRejection, FullscreenRenderPlanMetrics, RenderableSurface,
    ResolvedEffectScene, SceneNodeId,
};
use oblivion_one::effects::EffectRegion;

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SnapshotWorkCounters {
    pub(crate) snapshot_finalizations: usize,
    pub(crate) snapshot_owned_clones: usize,
    pub(crate) identity_computations: usize,
}

#[cfg(test)]
thread_local! {
    static SNAPSHOT_WORK_COUNTERS: std::cell::Cell<SnapshotWorkCounters> =
        const {
            std::cell::Cell::new(SnapshotWorkCounters {
                snapshot_finalizations: 0,
                snapshot_owned_clones: 0,
                identity_computations: 0,
            })
        };
}

#[cfg(test)]
pub(crate) fn reset_snapshot_work_counters() {
    SNAPSHOT_WORK_COUNTERS.with(|counters| counters.set(SnapshotWorkCounters::default()));
}

#[cfg(test)]
pub(crate) fn snapshot_work_counters() -> SnapshotWorkCounters {
    SNAPSHOT_WORK_COUNTERS.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn note_snapshot_finalization() {
    SNAPSHOT_WORK_COUNTERS.with(|counters| {
        let mut value = counters.get();
        value.snapshot_finalizations += 1;
        counters.set(value);
    });
}

#[cfg(test)]
pub(crate) fn note_snapshot_owned_clone() {
    SNAPSHOT_WORK_COUNTERS.with(|counters| {
        let mut value = counters.get();
        value.snapshot_owned_clones += 1;
        counters.set(value);
    });
}

#[cfg(test)]
pub(crate) fn note_identity_computation() {
    SNAPSHOT_WORK_COUNTERS.with(|counters| {
        let mut value = counters.get();
        value.identity_computations += 1;
        counters.set(value);
    });
}

/// A native frame projection keeps canonical surface identity paired with the
/// surface it qualified.  The pair is filtered together whenever a frame-local
/// projection needs to own a subset of the active scene.
pub(crate) fn filter_surface_scene_nodes<'a, F>(
    surfaces: Cow<'a, [RenderableSurface]>,
    scene_nodes: Cow<'a, [SceneNodeId]>,
    mut keep: F,
) -> (Cow<'a, [RenderableSurface]>, Cow<'a, [SceneNodeId]>)
where
    F: FnMut(&RenderableSurface) -> bool,
{
    debug_assert_eq!(surfaces.len(), scene_nodes.len());
    let mut filtered_surfaces = Vec::with_capacity(surfaces.len());
    let mut filtered_scene_nodes = Vec::with_capacity(scene_nodes.len());
    for (surface, scene_node) in surfaces.iter().zip(scene_nodes.iter().copied()) {
        if keep(surface) {
            filtered_surfaces.push(surface.clone());
            filtered_scene_nodes.push(scene_node);
        }
    }
    (
        Cow::Owned(filtered_surfaces),
        Cow::Owned(filtered_scene_nodes),
    )
}

pub(crate) fn assert_surface_scene_node_alignment(
    surfaces: &[RenderableSurface],
    scene_nodes: &[SceneNodeId],
) {
    debug_assert_eq!(
        surfaces.len(),
        scene_nodes.len(),
        "native frame surfaces and SceneNode IDs must have equal cardinality"
    );
}

pub(crate) fn visibility_signature(metrics: FullscreenRenderPlanMetrics) -> u64 {
    let mut signature = 0xcbf2_9ce4_8422_2325_u64;
    for value in [
        metrics.fullscreen_active as u64,
        u64::from(metrics.owner_root_surface_id.unwrap_or(0)),
        metrics.solitary_tree_active as u64,
        metrics.culled_surface_count as u64,
        metrics.wallpaper_culled as u64,
        metrics.visible_overlay_count as u64,
        metrics.rejection.map_or(0, |rejection| match rejection {
            FullscreenPresentationRejection::NoFullscreenOwner => 1,
            FullscreenPresentationRejection::OwnerMissing => 2,
            FullscreenPresentationRejection::OwnerMinimized => 3,
            FullscreenPresentationRejection::OwnerDoesNotCoverOutput => 4,
            FullscreenPresentationRejection::OwnerOpacityUnknown => 5,
            FullscreenPresentationRejection::OverlayVisible => 6,
            FullscreenPresentationRejection::SoftwareCursorVisible => 7,
            FullscreenPresentationRejection::TransformOrScaleIncompatible => 8,
        }),
    ] {
        signature ^= value;
        signature = signature.wrapping_mul(0x1000_0000_01b3);
    }
    signature
}

pub(crate) fn finalize_snapshot(
    mut snapshot: NativeSceneSnapshot,
    external_overlay_surface_ids: &[u32],
    visibility: FullscreenRenderPlanMetrics,
    effects: &ResolvedEffectScene,
) -> (NativeSceneSnapshot, u64) {
    snapshot.external_overlay_surface_ids = external_overlay_surface_ids.to_vec();
    snapshot.visibility_signature = visibility_signature(visibility);
    snapshot.effect_damage = effects
        .instances
        .iter()
        .fold(EffectRegion::empty(), |damage, instance| {
            damage.union(&instance.region)
        });
    snapshot.effect_identity_signature = effects.signature;
    let mut scene_identity_signature = snapshot.identity_signature();
    scene_identity_signature ^= effects.signature;
    scene_identity_signature = scene_identity_signature.wrapping_mul(0x1000_0000_01b3);
    (snapshot, scene_identity_signature)
}
