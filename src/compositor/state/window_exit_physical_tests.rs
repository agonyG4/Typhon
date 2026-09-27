use super::CompositorState;
use super::window_exit_physical::prove_window_exit_source;
use super::window_exit_retained::{
    PreparedWindowExit, WindowExitFrozenContent, WindowExitPainterOrder, WindowExitPayload,
    WindowExitPropertyRevisions, WindowExitReleaseObligation,
};
use crate::compositor::ExplicitSyncPoint;
use crate::compositor::surface::SurfaceCommitSequence;
use crate::compositor::{
    PresentedCanonicalSceneSnapshot, PresentedSurfaceContentEvidence, SurfacePresentationKey,
};
use crate::compositor::{
    RenderableSurface, RenderableSurfaceDamage, SurfacePlacement, WindowExitFrameEvidence,
};
use crate::core::{OutputId, SceneNodeId, WindowId};
use crate::presentation_animation::{
    AnimationCurve, AnimationTime, EasingCurve, PresentationClip, PresentationFrameSnapshot,
    PresentationGeometryMutation, PresentationOpacity, PresentationOpacityMutation,
    PresentationRetainedVisualIdentity, PresentationRetainedVisualKind,
    PresentationSampleTimeSource, PresentationTransactionMemberKind,
    PresentationTransactionRequest, PresentationWindowTarget,
};
use crate::render_backend::buffer::{
    BufferId, BufferIdAllocator, BufferSize, CommittedSurfaceBuffer,
};
use std::sync::Arc;
use std::time::Duration;

fn evidence(
    surface_id: u32,
    generation: u64,
    commit: u64,
    buffer_id: u64,
    scene_node_id: u64,
    visual_root_surface_id: u32,
    presentation_owner_root_surface_id: u32,
) -> PresentedSurfaceContentEvidence {
    PresentedSurfaceContentEvidence {
        key: SurfacePresentationKey {
            surface_id,
            generation,
        },
        commit_sequence: SurfaceCommitSequence(commit),
        buffer_id: BufferId::for_tests(buffer_id),
        scene_node_id: SceneNodeId::from_raw(scene_node_id).expect("test scene node"),
        visual_root_surface_id,
        presentation_owner_root_surface_id,
    }
}

fn fixture() -> (
    PresentedCanonicalSceneSnapshot,
    Vec<PresentedSurfaceContentEvidence>,
) {
    let output_id = OutputId::from_raw(1).expect("test output");
    let root = evidence(10, 7, 31, 400, 80, 10, 10);
    let child = evidence(11, 3, 15, 401, 80, 10, 10);
    let mut surfaces = vec![root, child];
    surfaces.sort_unstable_by_key(|surface| surface.key.surface_id);
    (
        PresentedCanonicalSceneSnapshot {
            output_id,
            render_generation: 50,
            effect_identity_signature: 900,
            surfaces: surfaces.clone(),
        },
        surfaces,
    )
}

fn shm_surface(surface_id: u32) -> RenderableSurface {
    let buffer_id = BufferIdAllocator::default()
        .allocate()
        .expect("test buffer id");
    RenderableSurface {
        surface_id,
        x: 0,
        y: 0,
        width: 4,
        height: 4,
        placement: SurfacePlacement::root_at(0, 0),
        render_backend: crate::compositor::SurfaceRenderBackend::NativeWayland,
        render_placement: None,
        visual_clip: None,
        render_target_size: None,
        generation: 1,
        commit_sequence: SurfaceCommitSequence::initial(),
        buffer: CommittedSurfaceBuffer::shm_snapshot(
            buffer_id,
            BufferSize::new(4, 4).expect("test buffer size"),
            vec![0; 16],
        ),
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        damage: RenderableSurfaceDamage::Full,
    }
}

fn activate_window_exit(
    state: &mut CompositorState,
    held_release_obligations: Vec<WindowExitReleaseObligation>,
) -> (PresentationRetainedVisualIdentity, WindowExitFrameEvidence) {
    let root_surface_id = 10;
    let window_id = WindowId::from_raw(10).expect("test WindowId");
    let scene_node_id = SceneNodeId::from_raw(80).expect("test WindowGroup SceneNodeId");
    let rect = crate::presentation_animation::PresentationRect::new(10.0, 20.0, 300.0, 200.0)
        .expect("test rectangle");
    let target_rect =
        crate::presentation_animation::PresentationRect::new(20.0, 20.0, 300.0, 200.0)
            .expect("test target rectangle");
    let started_at = AnimationTime::from_nanos(10);
    state.presentation_animator.set_enabled(true);
    let identity = state
        .presentation_animator
        .begin_retained_visual(
            scene_node_id,
            PresentationRetainedVisualKind::WindowExit,
            started_at,
        )
        .expect("reserve WindowExit identity");
    let prepared = PreparedWindowExit {
        content: Arc::new(WindowExitFrozenContent {
            window_id,
            root_surface_id,
            scene_node_id,
            surfaces: vec![shm_surface(root_surface_id)],
            surface_scene_node_ids: vec![scene_node_id],
            visual_root_surface_ids: vec![root_surface_id],
            presentation_owner_root_surface_ids: vec![root_surface_id],
            canonical_rect: rect,
            close_geometry_target: rect,
            close_curve: AnimationCurve::easing(Duration::from_millis(1), EasingCurve::EaseInCubic),
            source_presented_rect: rect,
            source_presented_opacity: PresentationOpacity::OPAQUE,
            source_presented_clip: PresentationClip::Unbounded,
            frozen_decoration: None,
            effect_scene: Arc::new(crate::compositor::ResolvedEffectScene::default()),
            painter_order: WindowExitPainterOrder {
                scene_band: 2,
                layer_rank: 2,
                stack_position: 0,
            },
            render_generation: 1,
            effect_identity_signature: 1,
        }),
        held_release_obligations,
    };
    let payload = WindowExitPayload::new(identity, prepared, started_at)
        .expect("valid frozen WindowExit payload");
    state
        .window_exit_payloads
        .publish_candidate_exact(identity, payload)
        .expect("publish candidate payload");
    let committed = state
        .presentation_animator
        .commit(PresentationTransactionRequest::mixed(
            started_at,
            vec![PresentationGeometryMutation::new(
                scene_node_id,
                rect,
                target_rect,
                AnimationCurve::easing(Duration::from_millis(1), EasingCurve::EaseInCubic),
            )],
            vec![PresentationOpacityMutation::new(
                scene_node_id,
                PresentationOpacity::OPAQUE,
                PresentationOpacity::TRANSPARENT,
                AnimationCurve::easing(Duration::from_millis(1), EasingCurve::EaseInCubic),
            )],
        ))
        .expect("commit exact Geometry+Opacity pair");
    let mut geometry_revision_id = None;
    let mut opacity_revision_id = None;
    for member in committed.members() {
        match member.kind() {
            PresentationTransactionMemberKind::Property(
                crate::presentation_animation::PresentationPropertyKind::Geometry,
            ) => geometry_revision_id = Some(member.revision_id()),
            PresentationTransactionMemberKind::Property(
                crate::presentation_animation::PresentationPropertyKind::Opacity,
            ) => opacity_revision_id = Some(member.revision_id()),
            _ => {}
        }
    }
    let revisions = WindowExitPropertyRevisions {
        transaction_id: committed.id(),
        geometry_revision_id: geometry_revision_id.expect("Geometry revision"),
        opacity_revision_id: opacity_revision_id.expect("Opacity revision"),
    };
    assert!(
        state
            .window_exit_payloads
            .set_property_revisions_before_activation(identity, revisions)
    );
    assert_eq!(
        state
            .presentation_animator
            .activate_retained_visual_exact(identity)
            .expect("activate retained WindowExit owner"),
        None
    );
    let evidence = WindowExitFrameEvidence {
        identity,
        payload_id: identity.revision_id().get(),
        root_surface_id,
        scene_node_id,
    };
    (identity, evidence)
}

#[test]
fn exact_promoted_surface_group_proves_the_window_exit_source() {
    let (promoted, current) = fixture();
    assert!(prove_window_exit_source(
        Some(&promoted),
        promoted.output_id,
        10,
        50,
        900,
        &current,
        true,
    ));
}

#[test]
fn missing_or_stale_physical_evidence_rejects_the_window_exit_source() {
    let (promoted, current) = fixture();
    assert!(!prove_window_exit_source(
        None,
        promoted.output_id,
        10,
        50,
        900,
        &current,
        true,
    ));
    assert!(!prove_window_exit_source(
        Some(&promoted),
        promoted.output_id,
        10,
        51,
        900,
        &current,
        true,
    ));
    assert!(!prove_window_exit_source(
        Some(&promoted),
        promoted.output_id,
        10,
        50,
        901,
        &current,
        true,
    ));
    assert!(!prove_window_exit_source(
        Some(&promoted),
        promoted.output_id,
        10,
        50,
        900,
        &current,
        false,
    ));
}

#[test]
fn any_surface_identity_or_membership_change_rejects_window_exit_capture() {
    let (promoted, current) = fixture();
    for mutation in 0..6 {
        let mut changed = current.clone();
        match mutation {
            0 => changed[0].key.generation += 1,
            1 => changed[0].commit_sequence.0 += 1,
            2 => changed[0].buffer_id = BufferId::for_tests(999),
            3 => changed[0].scene_node_id = SceneNodeId::from_raw(81).expect("test node"),
            4 => {
                changed.pop();
            }
            _ => changed.push(evidence(12, 1, 1, 402, 80, 10, 10)),
        }
        assert!(!prove_window_exit_source(
            Some(&promoted),
            promoted.output_id,
            10,
            50,
            900,
            &changed,
            true,
        ));
    }
}

#[test]
fn foreign_output_evidence_never_proves_window_exit_capture() {
    let (promoted, current) = fixture();
    assert!(!prove_window_exit_source(
        Some(&promoted),
        OutputId::from_raw(2).expect("foreign output"),
        10,
        50,
        900,
        &current,
        true,
    ));
}

#[test]
fn mathematically_settled_exit_waits_for_exact_physical_ack_before_retiring() {
    let mut state = CompositorState::new(None);
    let output_id = state.native_output_id().expect("test output id");
    let sync_point = ExplicitSyncPoint::for_tests_with_signal_script(99, 700, [true]);
    let obligation = crate::compositor::state_data::DmabufReleaseObligation {
        buffer_id: BufferId::for_tests(701),
        release: crate::compositor::state_data::SurfaceBufferRelease::ExplicitSync(
            sync_point.clone(),
        ),
    };
    let (identity, evidence) = activate_window_exit(
        &mut state,
        vec![WindowExitReleaseObligation {
            surface_id: evidence_surface_id(),
            obligation: obligation.clone(),
        }],
    );
    assert!(state.has_unowned_frame_work());
    assert!(
        state
            .direct_scanout_scene_blockers()
            .reasons()
            .contains(&crate::compositor::DirectScanoutSceneRejection::WindowExitAnimation)
    );
    assert!(state.buffer_release_is_owned(&obligation));
    let target = PresentationWindowTarget::with_scene_node(
        evidence.scene_node_id,
        evidence.root_surface_id,
        crate::presentation_animation::PresentationRect::new(10.0, 20.0, 300.0, 200.0)
            .expect("test target rect"),
    )
    .with_canonical_opacity(PresentationOpacity::OPAQUE)
    .with_canonical_clip(PresentationClip::Unbounded);
    let settled_sample = state.presentation_animator.sample(
        output_id,
        AnimationTime::from_nanos(2_000_010),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target],
    );
    let settled_snapshot = PresentationFrameSnapshot::from_sample(&settled_sample);

    state.settle_window_exit_physical(1, &settled_snapshot, &[evidence]);
    assert!(state.window_exit_payloads.get_exact(identity).is_some());
    assert!(state.buffer_release_is_owned(&obligation));
    assert_eq!(
        state.presentation_animator.active_retained_visual(
            evidence.scene_node_id,
            PresentationRetainedVisualKind::WindowExit,
        ),
        Some(identity),
        "mathematical settlement alone must not retire WindowExit"
    );

    state.publish_presented_presentation(1, &settled_snapshot);
    let mut stale_evidence = evidence;
    stale_evidence.payload_id = stale_evidence.payload_id.saturating_add(1);
    state.settle_window_exit_physical(1, &settled_snapshot, &[stale_evidence]);
    assert!(state.window_exit_payloads.get_exact(identity).is_some());
    assert!(state.buffer_release_is_owned(&obligation));

    state.settle_window_exit_physical(1, &settled_snapshot, &[evidence]);
    assert!(state.window_exit_payloads.get_exact(identity).is_none());
    assert!(
        state
            .pending_dmabuf_buffer_releases
            .iter()
            .any(|pending| pending.same_release_token(&obligation))
    );
    assert!(state.buffer_release_is_owned(&obligation));
    assert_eq!(
        sync_point
            .signal_script
            .as_ref()
            .expect("test sync signal script")
            .lock()
            .expect("test signal script lock")
            .len(),
        1,
        "physical retirement queues the exact release token without directly signaling it"
    );
    assert!(
        !state
            .presentation_animator
            .has_pending_visible(&[evidence.scene_node_id])
    );
    assert!(
        !state
            .direct_scanout_scene_blockers()
            .reasons()
            .contains(&crate::compositor::DirectScanoutSceneRejection::WindowExitAnimation)
    );
    assert_eq!(
        state.presentation_animator.active_retained_visual(
            evidence.scene_node_id,
            PresentationRetainedVisualKind::WindowExit,
        ),
        None
    );
}

#[test]
fn remap_retirement_cancels_close_tracks_and_returns_held_release_to_queue() {
    let mut state = CompositorState::new(None);
    let sync_point = ExplicitSyncPoint::for_tests_with_signal_script(99, 702, [true]);
    let obligation = crate::compositor::state_data::DmabufReleaseObligation {
        buffer_id: BufferId::for_tests(703),
        release: crate::compositor::state_data::SurfaceBufferRelease::ExplicitSync(
            sync_point.clone(),
        ),
    };
    let (identity, evidence) = activate_window_exit(
        &mut state,
        vec![WindowExitReleaseObligation {
            surface_id: evidence_surface_id(),
            obligation: obligation.clone(),
        }],
    );

    assert!(state.retire_window_exit_for_root(evidence.root_surface_id));
    assert!(state.window_exit_payloads.get_exact(identity).is_none());
    assert!(
        !state
            .presentation_animator
            .has_geometry_track(evidence.scene_node_id)
    );
    assert!(
        !state
            .presentation_animator
            .has_opacity_track(evidence.scene_node_id)
    );
    assert!(
        state
            .pending_dmabuf_buffer_releases
            .iter()
            .any(|pending| pending.same_release_token(&obligation))
    );
    assert_eq!(
        sync_point
            .signal_script
            .as_ref()
            .expect("test sync signal script")
            .lock()
            .expect("test signal script lock")
            .len(),
        1
    );
}

fn evidence_surface_id() -> u32 {
    10
}
