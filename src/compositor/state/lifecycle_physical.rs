use crate::compositor::PresentedLifecycleScene;
use crate::core::{SceneNodeId, WindowId};
use crate::window_lifecycle_animation::{
    LifecycleFrameLamp, LifecycleFrameSnapshot, lamp_footprint_intersects_output,
};

/// Evidence of lifecycle Lamps that may still occupy a physically presented
/// framebuffer. This is not logical active ownership and never acknowledges
/// or retires `PresentationEngine` state.
#[derive(Debug, Default)]
pub(crate) struct PresentedLifecyclePhysicalState {
    frame_id: u64,
    snapshot: LifecycleFrameSnapshot,
}

impl PresentedLifecyclePhysicalState {
    pub(crate) const fn frame_id(&self) -> u64 {
        self.frame_id
    }

    pub(crate) fn snapshot(&self) -> &LifecycleFrameSnapshot {
        &self.snapshot
    }

    pub(crate) fn publish(
        &mut self,
        frame_id: u64,
        snapshot: &LifecycleFrameSnapshot,
        scene: PresentedLifecycleScene<'_>,
        output_width: u32,
        output_height: u32,
    ) {
        self.frame_id = frame_id;
        let (canonical_root_surface_ids, rendered_scene_replacement) = match scene {
            PresentedLifecycleScene::Initial => (&[][..], false),
            PresentedLifecycleScene::RenderedSceneReplacement {
                canonical_root_surface_ids,
            } => (canonical_root_surface_ids, true),
        };
        let mut qualified = snapshot.clone();
        for old in &self.snapshot.lamps {
            let replaced = snapshot
                .lamps
                .iter()
                .any(|lamp| lamp.root_surface_id == old.root_surface_id);
            let canonical_replaced = canonical_root_surface_ids.contains(&old.root_surface_id);
            let rendered_replaced = rendered_scene_replacement && !replaced;
            if !replaced
                && !canonical_replaced
                && !rendered_replaced
                && pending_visible(old, output_width, output_height)
            {
                qualified.lamps.push(*old);
            }
        }
        qualified.refresh_signature();
        self.snapshot = qualified;
    }

    pub(crate) fn has_pending_visible(&self, output_width: u32, output_height: u32) -> bool {
        self.snapshot
            .lamps
            .iter()
            .any(|lamp| pending_visible(lamp, output_width, output_height))
    }

    pub(crate) fn has_pending_visible_scene_node(
        &self,
        scene_node_id: SceneNodeId,
        output_width: u32,
        output_height: u32,
    ) -> bool {
        self.snapshot.lamps.iter().any(|lamp| {
            lamp.presentation_identity.scene_node_id() == scene_node_id
                && pending_visible(lamp, output_width, output_height)
        })
    }

    pub(crate) fn remove_window(&mut self, window_id: WindowId) {
        self.snapshot
            .lamps
            .retain(|lamp| lamp.window_id != window_id);
        self.snapshot.refresh_signature();
    }

    #[cfg(test)]
    pub(crate) fn snapshot_for_test(&self) -> &LifecycleFrameSnapshot {
        &self.snapshot
    }

    #[cfg(test)]
    pub(crate) fn seed_snapshot_for_test(&mut self, snapshot: LifecycleFrameSnapshot) {
        self.snapshot = snapshot;
        self.snapshot.refresh_signature();
    }
}

fn pending_visible(lamp: &LifecycleFrameLamp, output_width: u32, output_height: u32) -> bool {
    !lamp.mathematically_settled
        && lamp.opacity > f64::EPSILON
        && lamp_footprint_intersects_output(lamp.visual_group, output_width, output_height)
}
