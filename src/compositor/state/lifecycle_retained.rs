use crate::compositor::state::RetainedSurfacePresentationSnapshot;
use crate::compositor::{
    DecorationRenderInstance, DmabufReleaseObligation, RenderableSurface, ResolvedEffectScene,
    WindowId,
};
use crate::core::SceneNodeId;
use crate::presentation_animation::{
    PresentationRetainedVisualIdentity, PresentationRetainedVisualKind, PresentationRevisionId,
};
use crate::window_lifecycle_animation::{LifecycleVisualGroup, valid_lifecycle_visual_group};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

const MAX_RETAINED_LIFECYCLE_ENTRIES: usize = 256;
pub(crate) const MAX_LIFECYCLE_PRECAPTURE_SURFACES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum LifecyclePreCaptureEvent {
    LeaseCreated,
    SourceWithdrawnWhileLeased,
    TransferredToFrame,
    ReleasedAfterCapture,
    RestoredToLiveOwner,
    QueuedAfterCancel,
    DrainedOnShutdown,
}

#[derive(Debug)]
pub(crate) struct LifecycleHeldReleaseObligation {
    pub(crate) surface_id: u32,
    pub(crate) obligation: DmabufReleaseObligation,
}

/// Admission-time source ownership. This lease ends at successful renderer
/// capture or lifecycle retirement; reversal moves it with its retained entry.
#[derive(Debug)]
pub(crate) struct LifecyclePreCaptureLease {
    pub(crate) surfaces: Vec<RenderableSurface>,
    pub(crate) held_release_obligations: Vec<LifecycleHeldReleaseObligation>,
}

impl LifecyclePreCaptureLease {
    pub(crate) fn new(
        surfaces: Vec<RenderableSurface>,
        held_release_obligations: Vec<LifecycleHeldReleaseObligation>,
    ) -> Option<Self> {
        if surfaces.is_empty()
            || surfaces.len() > MAX_LIFECYCLE_PRECAPTURE_SURFACES
            || held_release_obligations.len() > surfaces.len()
        {
            return None;
        }
        Some(Self {
            surfaces,
            held_release_obligations,
        })
    }

    pub(crate) fn dmabuf_surface_count(&self) -> usize {
        self.surfaces
            .iter()
            .filter(|surface| surface.dmabuf_handle().is_some())
            .count()
    }
}

#[derive(Debug)]
pub(crate) struct RetainedLifecycleEntry {
    pub(crate) payload: Arc<RetainedLifecyclePayload>,
    pub(crate) pre_capture: Option<LifecyclePreCaptureLease>,
}

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

/// Frozen compositor metadata, admission-time client-surface topology, and
/// resolved owned effects for one lifecycle chain.
#[derive(Debug, Clone)]
pub(crate) struct RetainedLifecyclePayload {
    pub(crate) payload_id: PresentationRetainedVisualPayloadId,
    pub(crate) window_id: WindowId,
    pub(crate) root_surface_id: u32,
    pub(crate) visual_group: LifecycleVisualGroup,
    pub(crate) effect_scene: Arc<ResolvedEffectScene>,
    pub(crate) frozen_decoration: Option<DecorationRenderInstance>,
    pub(crate) surface_presentation: RetainedSurfacePresentationSnapshot,
}

impl RetainedLifecyclePayload {
    pub(crate) fn capture(
        origin_identity: PresentationRetainedVisualIdentity,
        window_id: WindowId,
        root_surface_id: u32,
        visual_group: LifecycleVisualGroup,
        effect_scene: ResolvedEffectScene,
        frozen_decoration: Option<DecorationRenderInstance>,
        surface_presentation: RetainedSurfacePresentationSnapshot,
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
            frozen_decoration,
            surface_presentation,
        }))
    }
}

/// Exact identity lookup for immutable lifecycle payload. Active ownership
/// remains exclusively in PresentationEngine.
#[derive(Debug, Default)]
pub(crate) struct RetainedLifecyclePayloadStore {
    entries: BTreeMap<PresentationRetainedVisualIdentity, RetainedLifecycleEntry>,
}

impl RetainedLifecyclePayloadStore {
    pub(crate) fn get_exact(
        &self,
        identity: PresentationRetainedVisualIdentity,
    ) -> Option<&Arc<RetainedLifecyclePayload>> {
        self.entries.get(&identity).map(|entry| &entry.payload)
    }

    pub(crate) fn get_entry_exact(
        &self,
        identity: PresentationRetainedVisualIdentity,
    ) -> Option<&RetainedLifecycleEntry> {
        self.entries.get(&identity)
    }

    pub(crate) fn get_entry_exact_mut(
        &mut self,
        identity: PresentationRetainedVisualIdentity,
    ) -> Option<&mut RetainedLifecycleEntry> {
        self.entries.get_mut(&identity)
    }

    pub(crate) fn entries_for_source_withdrawal(
        &self,
        surface_ids: &HashSet<u32>,
    ) -> Vec<(
        PresentationRetainedVisualIdentity,
        Arc<RetainedLifecyclePayload>,
        usize,
        usize,
        usize,
    )> {
        self.entries
            .iter()
            .filter_map(|(identity, entry)| {
                let lease = entry.pre_capture.as_ref()?;
                lease
                    .surfaces
                    .iter()
                    .any(|surface| surface_ids.contains(&surface.surface_id))
                    .then(|| {
                        (
                            *identity,
                            Arc::clone(&entry.payload),
                            lease.surfaces.len(),
                            lease.dmabuf_surface_count(),
                            lease.held_release_obligations.len(),
                        )
                    })
            })
            .collect()
    }

    pub(crate) fn precapture_dmabuf_release_count(&self) -> usize {
        self.entries
            .values()
            .filter_map(|entry| entry.pre_capture.as_ref())
            .map(|lease| lease.held_release_obligations.len())
            .sum()
    }

    pub(crate) fn windows_with_precapture(&self) -> Vec<WindowId> {
        let mut windows = Vec::new();
        for entry in self.entries.values() {
            if entry.pre_capture.is_some() && !windows.contains(&entry.payload.window_id) {
                windows.push(entry.payload.window_id);
            }
        }
        windows
    }

    pub(crate) fn holds_release_token(&self, candidate: &DmabufReleaseObligation) -> bool {
        self.entries.values().any(|entry| {
            entry.pre_capture.as_ref().is_some_and(|lease| {
                lease
                    .held_release_obligations
                    .iter()
                    .any(|held| held.obligation.same_release_token(candidate))
            })
        })
    }

    pub(crate) fn can_publish_exact(&self, identity: PresentationRetainedVisualIdentity) -> bool {
        !self.entries.contains_key(&identity) && self.entries.len() < MAX_RETAINED_LIFECYCLE_ENTRIES
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
                .is_some_and(|previous| Arc::ptr_eq(&previous.payload, payload))
            && self.entries.len() <= MAX_RETAINED_LIFECYCLE_ENTRIES
    }

    pub(crate) fn publish_exact(
        &mut self,
        identity: PresentationRetainedVisualIdentity,
        entry: RetainedLifecycleEntry,
    ) -> Result<(), RetainedLifecycleEntry> {
        if !self.can_publish_exact(identity) {
            return Err(entry);
        }
        self.entries.insert(identity, entry);
        Ok(())
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
        let Some(entry) = self.entries.remove(&previous_identity) else {
            return false;
        };
        self.entries.insert(next_identity, entry);
        true
    }

    pub(crate) fn retire_exact(
        &mut self,
        identity: PresentationRetainedVisualIdentity,
    ) -> Option<RetainedLifecycleEntry> {
        self.entries.remove(&identity)
    }

    pub(crate) fn remove_scene_node(
        &mut self,
        scene_node_id: SceneNodeId,
    ) -> Vec<(PresentationRetainedVisualIdentity, RetainedLifecycleEntry)> {
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
    ) -> Vec<(PresentationRetainedVisualIdentity, RetainedLifecycleEntry)> {
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
            None,
            RetainedSurfacePresentationSnapshot::test_root(root_surface_id),
        )
        .expect("valid payload")
    }

    fn entry(payload: Arc<RetainedLifecyclePayload>) -> RetainedLifecycleEntry {
        RetainedLifecycleEntry {
            payload,
            pre_capture: None,
        }
    }

    #[test]
    fn duplicate_exact_identity_does_not_replace_immutable_payload() {
        let mut engine = PresentationEngine::default();
        let identity = reserve_identity(&mut engine);
        let first = payload(identity, 10);
        let replacement = payload(identity, 20);
        let mut store = RetainedLifecyclePayloadStore::default();

        assert!(
            store
                .publish_exact(identity, entry(Arc::clone(&first)))
                .is_ok()
        );
        assert!(store.publish_exact(identity, entry(replacement)).is_err());

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
        assert!(
            store
                .publish_exact(first_identity, entry(Arc::clone(&frozen)))
                .is_ok()
        );

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
                .publish_exact(
                    payload_only_identity,
                    entry(payload(payload_only_identity, 10)),
                )
                .is_ok()
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
                    effect: crate::window_lifecycle_animation::LifecycleEffectKind::Lamp,
                    canonical_opacity: 1.0,
                },
                None,
                AnimationTime::from_nanos(0),
                1.0,
            )
            .expect("motion-only executor entry");

        let sample = state.lifecycle_scene_sample_at(AnimationTime::from_nanos(1));
        assert!(sample.samples.is_empty());
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
