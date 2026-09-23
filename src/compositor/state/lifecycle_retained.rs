use crate::compositor::{ResolvedEffectScene, WindowId};
use crate::core::SceneNodeId;
use crate::presentation_animation::{
    PresentationRetainedVisualIdentity, PresentationRetainedVisualKind, PresentationRevisionId,
};
use crate::window_lifecycle_animation::{LifecycleVisualGroup, valid_lifecycle_visual_group};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Immutable-content continuity across exact retained presentation revisions.
/// This token qualifies a frozen visual source; it is never an active owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PresentationRetainedVisualPayloadId(PresentationRevisionId);

impl PresentationRetainedVisualPayloadId {
    /// Derives the immutable-content token from the first exact owner in a
    /// retained lifecycle chain. Reversals keep this value unchanged.
    pub const fn from_origin_identity(identity: PresentationRetainedVisualIdentity) -> Self {
        Self(identity.revision_id())
    }

    /// Returns the underlying globally unique revision token.
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Frozen compositor metadata and resolved owned effects for one lifecycle
/// chain. Client surfaces remain live in the canonical compositor scene.
#[derive(Debug, Clone)]
pub(crate) struct RetainedLifecyclePayload {
    pub(crate) payload_id: PresentationRetainedVisualPayloadId,
    pub(crate) window_id: WindowId,
    pub(crate) root_surface_id: u32,
    pub(crate) visual_group: LifecycleVisualGroup,
    pub(crate) effect_scene: Arc<ResolvedEffectScene>,
}

impl RetainedLifecyclePayload {
    pub(crate) fn capture(
        origin_identity: PresentationRetainedVisualIdentity,
        window_id: WindowId,
        root_surface_id: u32,
        visual_group: LifecycleVisualGroup,
        effect_scene: ResolvedEffectScene,
    ) -> Option<Arc<Self>> {
        if origin_identity.kind() != PresentationRetainedVisualKind::WindowLifecycle
            || !valid_lifecycle_visual_group(visual_group)
        {
            return None;
        }
        Some(Arc::new(Self {
            payload_id: PresentationRetainedVisualPayloadId::from_origin_identity(origin_identity),
            window_id,
            root_surface_id,
            visual_group,
            effect_scene: Arc::new(effect_scene),
        }))
    }
}

/// Exact identity lookup for immutable lifecycle payload. Active ownership
/// remains exclusively in PresentationEngine.
#[derive(Debug, Default)]
pub(crate) struct RetainedLifecyclePayloadStore {
    entries: BTreeMap<PresentationRetainedVisualIdentity, Arc<RetainedLifecyclePayload>>,
}

impl RetainedLifecyclePayloadStore {
    pub(crate) fn get_exact(
        &self,
        identity: PresentationRetainedVisualIdentity,
    ) -> Option<&Arc<RetainedLifecyclePayload>> {
        self.entries.get(&identity)
    }

    pub(crate) fn can_publish_exact(&self, identity: PresentationRetainedVisualIdentity) -> bool {
        !self.entries.contains_key(&identity)
    }

    pub(crate) fn can_transfer_exact(
        &self,
        previous_identity: PresentationRetainedVisualIdentity,
        next_identity: PresentationRetainedVisualIdentity,
        payload: &Arc<RetainedLifecyclePayload>,
    ) -> bool {
        !self.entries.contains_key(&next_identity)
            && self
                .entries
                .get(&previous_identity)
                .is_some_and(|previous| Arc::ptr_eq(previous, payload))
    }

    pub(crate) fn publish_exact(
        &mut self,
        identity: PresentationRetainedVisualIdentity,
        payload: Arc<RetainedLifecyclePayload>,
    ) -> bool {
        if self.entries.contains_key(&identity) {
            return false;
        }
        self.entries.insert(identity, payload);
        true
    }

    pub(crate) fn transfer_exact(
        &mut self,
        previous_identity: PresentationRetainedVisualIdentity,
        next_identity: PresentationRetainedVisualIdentity,
        payload: &Arc<RetainedLifecyclePayload>,
    ) -> bool {
        if !self.can_transfer_exact(previous_identity, next_identity, payload) {
            return false;
        }
        self.entries.insert(next_identity, Arc::clone(payload));
        self.entries.remove(&previous_identity);
        true
    }

    pub(crate) fn retire_exact(
        &mut self,
        identity: PresentationRetainedVisualIdentity,
    ) -> Option<Arc<RetainedLifecyclePayload>> {
        self.entries.remove(&identity)
    }

    pub(crate) fn remove_scene_node(
        &mut self,
        scene_node_id: SceneNodeId,
    ) -> Vec<(
        PresentationRetainedVisualIdentity,
        Arc<RetainedLifecyclePayload>,
    )> {
        let identities = self
            .entries
            .keys()
            .copied()
            .filter(|identity| identity.scene_node_id() == scene_node_id)
            .collect::<Vec<_>>();
        identities
            .into_iter()
            .filter_map(|identity| {
                self.retire_exact(identity)
                    .map(|payload| (identity, payload))
            })
            .collect()
    }

    pub(crate) fn drain_all(
        &mut self,
    ) -> Vec<(
        PresentationRetainedVisualIdentity,
        Arc<RetainedLifecyclePayload>,
    )> {
        std::mem::take(&mut self.entries).into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::CompositorState;
    use crate::core::SceneNodeId;
    use crate::presentation_animation::{AnimationTime, PresentationEngine};
    use crate::window_lifecycle_animation::{LifecycleDirection, LifecycleMotionRequest};

    fn reserve_identity(engine: &mut PresentationEngine) -> PresentationRetainedVisualIdentity {
        engine
            .begin_retained_visual(
                SceneNodeId::from_raw(1).expect("test SceneNodeId"),
                PresentationRetainedVisualKind::WindowLifecycle,
                AnimationTime::from_nanos(0),
            )
            .expect("retained identity")
    }

    fn payload(
        identity: PresentationRetainedVisualIdentity,
        root_surface_id: u32,
    ) -> Arc<RetainedLifecyclePayload> {
        let window_id = WindowId::from_raw(1).expect("test WindowId");
        let rect = |x, y, width, height| {
            crate::presentation_animation::PresentationRect::new(x, y, width, height)
                .expect("test presentation rect")
        };
        let visual_group = LifecycleVisualGroup::from_bounds(
            rect(20.0, 30.0, 200.0, 100.0),
            rect(20.0, 30.0, 200.0, 100.0),
            rect(20.0, 30.0, 200.0, 100.0),
            rect(800.0, 700.0, 80.0, 40.0),
            1920,
            1080,
        )
        .expect("test visual group");
        RetainedLifecyclePayload::capture(
            identity,
            window_id,
            root_surface_id,
            visual_group,
            ResolvedEffectScene::default(),
        )
        .expect("valid payload")
    }

    #[test]
    fn duplicate_exact_identity_does_not_replace_immutable_payload() {
        let mut engine = PresentationEngine::default();
        let identity = reserve_identity(&mut engine);
        let first = payload(identity, 10);
        let replacement = payload(identity, 20);
        let mut store = RetainedLifecyclePayloadStore::default();

        assert!(store.publish_exact(identity, Arc::clone(&first)));
        assert!(!store.publish_exact(identity, replacement));

        assert_eq!(store.get_exact(identity).unwrap().root_surface_id, 10);
        assert!(Arc::ptr_eq(store.get_exact(identity).unwrap(), &first));
    }

    #[test]
    fn reversal_transfer_preserves_payload_id_and_arc() {
        let mut engine = PresentationEngine::default();
        let first_identity = reserve_identity(&mut engine);
        let next_identity = reserve_identity(&mut engine);
        let frozen = payload(first_identity, 10);
        let mut store = RetainedLifecyclePayloadStore::default();
        assert!(store.publish_exact(first_identity, Arc::clone(&frozen)));

        assert!(store.transfer_exact(first_identity, next_identity, &frozen));

        assert!(store.get_exact(first_identity).is_none());
        let transferred = store.get_exact(next_identity).unwrap();
        assert_eq!(transferred.payload_id, frozen.payload_id);
        assert!(Arc::ptr_eq(transferred, &frozen));
    }

    #[test]
    fn independent_identity_uses_a_distinct_payload_id() {
        let mut engine = PresentationEngine::default();
        let first_identity = reserve_identity(&mut engine);
        let second_identity = reserve_identity(&mut engine);
        let first = payload(first_identity, 10);
        let second = payload(second_identity, 10);

        assert_ne!(first.payload_id, second.payload_id);
    }

    #[test]
    fn incomplete_active_lifecycle_pairs_do_not_render_or_block_work() {
        let mut state = CompositorState::new(None);
        state.lifecycle_animation_renderer_available = Some(true);

        let payload_only_identity = reserve_identity(&mut state.presentation_animator);
        state
            .presentation_animator
            .activate_retained_visual_exact(payload_only_identity)
            .expect("activate payload-only owner");
        assert!(
            state
                .retained_lifecycle_payloads
                .publish_exact(payload_only_identity, payload(payload_only_identity, 10),)
        );

        let motion_only_identity = state
            .presentation_animator
            .begin_retained_visual(
                SceneNodeId::from_raw(2).expect("test SceneNodeId"),
                PresentationRetainedVisualKind::WindowLifecycle,
                AnimationTime::from_nanos(0),
            )
            .expect("motion-only reservation");
        state
            .presentation_animator
            .activate_retained_visual_exact(motion_only_identity)
            .expect("activate motion-only owner");
        state
            .window_lifecycle_animator
            .start_or_reverse(
                LifecycleMotionRequest {
                    presentation_identity: motion_only_identity,
                    direction: LifecycleDirection::Minimize,
                },
                None,
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("motion-only executor entry");

        let sample = state.lifecycle_scene_sample_at(AnimationTime::from_nanos(1));
        assert!(sample.lamps.is_empty());
        assert!(sample.visual_sources.is_empty());
        assert!(!state.lifecycle_animation_has_pending_visible());
        assert!(!state.has_unowned_frame_work());
        assert!(
            !state
                .direct_scanout_scene_blockers()
                .reasons()
                .contains(&crate::compositor::DirectScanoutSceneRejection::LifecycleAnimation)
        );
    }
}
