use super::*;
use crate::core::{OutputId, SceneNodeId};
use std::num::NonZeroU64;
use std::time::Duration;

fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
    PresentationRect::new(x, y, width, height).expect("valid presentation rect")
}

fn clip_rect(x: f64, y: f64, width: f64, height: f64) -> PresentationClipRect {
    PresentationClipRect::new(x, y, width, height).expect("valid presentation clip rect")
}

fn clip_mutation(
    scene_node_id: SceneNodeId,
    start: PresentationClip,
    target: PresentationClip,
    envelope: Option<PresentationClipRect>,
    curve: AnimationCurve,
) -> PresentationClipMutation {
    PresentationClipMutation::new(scene_node_id, start, target, envelope, curve)
}

fn node(raw: u64) -> SceneNodeId {
    SceneNodeId::from_raw(raw).expect("nonzero scene node")
}

fn target(
    scene_node_id: SceneNodeId,
    root_surface_id: u32,
    rect: PresentationRect,
) -> PresentationWindowTarget {
    PresentationWindowTarget::with_scene_node(scene_node_id, root_surface_id, rect)
}

fn geometry_ack(
    output_id: OutputId,
    scene_node_id: SceneNodeId,
    transaction_id: PresentationTransactionId,
    revision_id: PresentationRevisionId,
    presented_rect: PresentationRect,
) -> PresentedGeometryAck {
    PresentedGeometryAck {
        output_id,
        scene_node_id,
        property: PresentationPropertyKind::Geometry,
        transaction_id,
        revision_id,
        presented_rect,
    }
}

fn opacity_ack(
    output_id: OutputId,
    scene_node_id: SceneNodeId,
    transaction_id: PresentationTransactionId,
    revision_id: PresentationRevisionId,
    presented_opacity: f64,
) -> PresentedOpacityAck {
    PresentedOpacityAck {
        output_id,
        scene_node_id,
        property: PresentationPropertyKind::Opacity,
        transaction_id,
        revision_id,
        presented_opacity: PresentationOpacity::new(presented_opacity).expect("valid test opacity"),
    }
}

fn clip_ack(
    output_id: OutputId,
    scene_node_id: SceneNodeId,
    transaction_id: PresentationTransactionId,
    revision_id: PresentationRevisionId,
    presented_clip: PresentationClip,
) -> PresentedClipAck {
    PresentedClipAck {
        output_id,
        scene_node_id,
        property: PresentationPropertyKind::Clip,
        transaction_id,
        revision_id,
        presented_clip,
    }
}

#[test]
fn opacity_transaction_uses_exact_revision_and_sample_evidence() {
    let mut engine = PresentationEngine::enabled();
    let transaction = engine
        .commit(PresentationTransactionRequest::opacity(
            AnimationTime::from_nanos(0),
            vec![PresentationOpacityMutation::new(
                node(101),
                PresentationOpacity::OPAQUE,
                PresentationOpacity::new(0.5).expect("opacity"),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("opacity transaction");
    let frame = engine.sample(
        OutputId::from_raw(1).expect("output"),
        AnimationTime::from_nanos(5_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target(node(101), 101, rect(0.0, 0.0, 10.0, 10.0))],
    );
    let opacity = frame.opacities[0];
    let transition = opacity.transition.expect("active opacity evidence");
    assert_eq!(transition.transaction_id, transaction.id());
    assert_eq!(
        transition.revision_id,
        transaction.members()[0].revision_id()
    );
    assert_eq!(
        opacity.opacity,
        PresentationOpacity::new(0.75).expect("opacity")
    );
}

#[test]
fn clip_transaction_projects_with_geometry_sample_from_same_timestamp() {
    let mut engine = PresentationEngine::enabled();
    let scene_node_id = node(119);
    let curve = AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear);
    let start_clip = PresentationClip::Rect(clip_rect(10.0, 5.0, 20.0, 10.0));
    let target_clip = PresentationClip::Rect(clip_rect(30.0, 20.0, 60.0, 20.0));
    let transaction = engine
        .commit(PresentationTransactionRequest::mixed_all(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                scene_node_id,
                rect(100.0, 100.0, 100.0, 100.0),
                rect(200.0, 150.0, 200.0, 300.0),
                curve,
            )],
            Vec::new(),
            vec![clip_mutation(
                scene_node_id,
                start_clip,
                target_clip,
                None,
                curve,
            )],
        ))
        .expect("geometry and clip transaction");
    assert_eq!(transaction.members().len(), 2);
    assert_ne!(
        transaction.members()[0].revision_id(),
        transaction.members()[1].revision_id()
    );

    let frame = engine.sample(
        OutputId::from_raw(1).expect("output"),
        AnimationTime::from_nanos(5_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target(scene_node_id, 119, rect(100.0, 100.0, 100.0, 100.0))],
    );
    assert_eq!(
        frame.clips[0].clip,
        PresentationClip::Rect(clip_rect(20.0, 12.5, 40.0, 15.0))
    );
    assert_eq!(
        frame.clips[0].presented_clip,
        Some(clip_rect(180.0, 150.0, 60.0, 30.0))
    );
}

#[test]
fn unbounded_clip_endpoints_remain_semantic_and_identity_signatures_stay_sparse() {
    let mut engine = PresentationEngine::enabled();
    let scene_node_id = node(121);
    let output = OutputId::from_raw(1).expect("output");
    let envelope = clip_rect(-200.0, -100.0, 600.0, 400.0);
    let rect_clip = PresentationClip::Rect(clip_rect(10.0, 20.0, 100.0, 80.0));
    let _transaction = engine
        .commit(PresentationTransactionRequest::clip(
            AnimationTime::from_nanos(0),
            vec![clip_mutation(
                scene_node_id,
                PresentationClip::Unbounded,
                rect_clip,
                Some(envelope),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("unbounded to rect");
    let target_window = target(scene_node_id, 121, rect(50.0, 60.0, 300.0, 200.0));
    let start = engine.sample(
        output,
        AnimationTime::from_nanos(0),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target_window],
    );
    assert_eq!(start.clips[0].clip, PresentationClip::Unbounded);
    assert_eq!(start.clips[0].presented_clip, None);
    assert!(start.clips[0].transition.is_some());

    let settled = engine.sample(
        output,
        AnimationTime::from_nanos(10_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target_window],
    );
    let settled_clip = settled.clips[0];
    assert_eq!(settled_clip.clip, rect_clip);
    assert!(
        settled_clip
            .transition
            .is_some_and(|transition| transition.mathematically_settled)
    );
    let ack = PresentedClipAck::from_group_clip(output, settled_clip).expect("active track ack");
    assert!(engine.acknowledge_presented_clip(output, ack));

    let reverse = engine
        .commit(PresentationTransactionRequest::clip(
            AnimationTime::from_nanos(20_000_000),
            vec![clip_mutation(
                scene_node_id,
                rect_clip,
                PresentationClip::Unbounded,
                Some(envelope),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("rect to unbounded");
    let unbounded = engine.sample(
        output,
        AnimationTime::from_nanos(30_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target_window],
    );
    assert_eq!(unbounded.clips[0].clip, PresentationClip::Unbounded);
    assert_eq!(unbounded.clips[0].presented_clip, None);
    assert!(
        unbounded.clips[0]
            .transition
            .is_some_and(|transition| transition.mathematically_settled)
    );
    assert!(engine.acknowledge_presented_clip(
        output,
        PresentedClipAck::from_group_clip(output, unbounded.clips[0]).expect("unbounded ack"),
    ));
    assert_eq!(engine.transaction_count(), 0);
    assert_eq!(reverse.members().len(), 1);

    let static_identity = engine.sample(
        output,
        AnimationTime::from_nanos(40_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target_window],
    );
    assert!(static_identity.clips.is_empty());
    assert_eq!(static_identity.presentation_visual_signature(), {
        let active_identity = PresentationSceneSample {
            clips: vec![PresentationGroupClip::with_scene_node(
                scene_node_id,
                121,
                PresentationClip::Unbounded,
                None,
                Some(PresentationClipTransitionEvidence {
                    transaction_id: reverse.id(),
                    revision_id: reverse.members()[0].revision_id(),
                    mathematically_settled: true,
                }),
            )],
            ..static_identity.clone()
        };
        active_identity.presentation_visual_signature()
    });
}

#[test]
fn invalid_clip_member_does_not_partially_commit_geometry() {
    let mut engine = PresentationEngine::enabled();
    let result = engine.commit(PresentationTransactionRequest::mixed_all(
        AnimationTime::from_nanos(0),
        vec![PresentationGeometryMutation::new(
            node(122),
            rect(0.0, 0.0, 10.0, 10.0),
            rect(10.0, 0.0, 10.0, 10.0),
            AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
        )],
        Vec::new(),
        vec![clip_mutation(
            node(122),
            PresentationClip::Unbounded,
            PresentationClip::Rect(clip_rect(0.0, 0.0, 10.0, 10.0)),
            None,
            AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
        )],
    ));
    assert_eq!(
        result,
        Err(PresentationTransactionError::MissingClipEnvelope)
    );
    assert_eq!(engine.active_count(), 0);
    assert_eq!(engine.transaction_count(), 0);
}

#[test]
fn clip_retarget_preserves_position_velocity_and_same_settled_target_revision() {
    let mut engine = PresentationEngine::enabled();
    let scene_node_id = node(123);
    let spring = AnimationCurve::spring(SpringSpec::new(100.0, 16.0));
    let first = engine
        .commit(PresentationTransactionRequest::clip(
            AnimationTime::from_nanos(0),
            vec![clip_mutation(
                scene_node_id,
                PresentationClip::Rect(clip_rect(0.0, 0.0, 10.0, 10.0)),
                PresentationClip::Rect(clip_rect(100.0, 20.0, 30.0, 40.0)),
                None,
                spring,
            )],
        ))
        .expect("first clip transition");
    let at = AnimationTime::from_nanos(120_000_000);
    let before = engine
        .sample_clip_for_scene_node(scene_node_id, at)
        .expect("sample before retarget");
    let retarget = engine
        .commit(PresentationTransactionRequest::clip(
            at,
            vec![clip_mutation(
                scene_node_id,
                PresentationClip::Rect(clip_rect(-50.0, -50.0, 5.0, 5.0)),
                PresentationClip::Rect(clip_rect(20.0, 80.0, 20.0, 20.0)),
                None,
                AnimationCurve::easing(Duration::from_millis(100), EasingCurve::EaseOut),
            )],
        ))
        .expect("clip retarget");
    let after = engine
        .sample_clip_for_scene_node(scene_node_id, at)
        .expect("sample after retarget");
    assert_eq!(after, before);
    assert_eq!(
        engine.clip_track_transaction(scene_node_id),
        Some(retarget.id())
    );
    assert_ne!(
        retarget.members()[0].revision_id(),
        first.members()[0].revision_id()
    );

    let output = OutputId::from_raw(1).expect("output");
    let target_window = target(scene_node_id, 123, rect(0.0, 0.0, 100.0, 100.0));
    let settled = engine.sample(
        output,
        AnimationTime::from_nanos(1_000_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target_window],
    );
    let revision = settled.clips[0]
        .transition
        .expect("settled transition")
        .revision_id;
    let same_target = engine.commit(PresentationTransactionRequest::clip(
        AnimationTime::from_nanos(1_100_000_000),
        vec![clip_mutation(
            scene_node_id,
            PresentationClip::Rect(clip_rect(0.0, 0.0, 10.0, 10.0)),
            PresentationClip::Rect(clip_rect(20.0, 80.0, 20.0, 20.0)),
            None,
            spring,
        )],
    ));
    assert_eq!(same_target, Err(PresentationTransactionError::Empty));
    assert_eq!(engine.clip_track_revision(scene_node_id), Some(revision));
}

#[test]
fn clip_zero_area_and_spring_lower_bound_are_visible_and_bounded() {
    let mut engine = PresentationEngine::enabled();
    let scene_node_id = node(124);
    let zero = PresentationClip::Rect(clip_rect(4.0, 5.0, 0.0, 8.0));
    let spring = AnimationCurve::spring(SpringSpec::new(100.0, 1.0));
    engine
        .commit(PresentationTransactionRequest::clip(
            AnimationTime::from_nanos(0),
            vec![clip_mutation(
                scene_node_id,
                PresentationClip::Rect(clip_rect(0.0, 0.0, 1.0, 1.0)),
                zero,
                None,
                spring,
            )],
        ))
        .expect("zero dimension spring");
    let sample = engine
        .sample_clip_for_scene_node(scene_node_id, AnimationTime::from_nanos(300_000_000))
        .expect("spring sample");
    let rect = sample.0.rect().expect("intermediate clip rect");
    assert!(rect.width() >= 0.0 && rect.height() >= 0.0);
    if rect.width() == 0.0 {
        assert_eq!(sample.1.width(), 0.0);
    }
}

#[test]
fn property_specific_track_queries_do_not_cross_properties() {
    let mut engine = PresentationEngine::enabled();
    let scene_node_id = node(120);
    let curve = AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear);

    engine
        .commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                scene_node_id,
                rect(0.0, 0.0, 10.0, 10.0),
                rect(1.0, 0.0, 10.0, 10.0),
                curve,
            )],
        ))
        .expect("geometry transaction");
    assert!(engine.has_geometry_track(scene_node_id));
    assert!(!engine.has_opacity_track(scene_node_id));

    engine.cancel_geometry(scene_node_id);
    engine
        .commit(PresentationTransactionRequest::opacity(
            AnimationTime::from_nanos(0),
            vec![PresentationOpacityMutation::new(
                scene_node_id,
                PresentationOpacity::OPAQUE,
                PresentationOpacity::new(0.5).expect("opacity"),
                curve,
            )],
        ))
        .expect("opacity transaction");
    assert!(!engine.has_geometry_track(scene_node_id));
    assert!(engine.has_opacity_track(scene_node_id));
}

#[test]
fn geometry_and_opacity_share_transaction_but_settle_independently() {
    let mut engine = PresentationEngine::enabled();
    let transaction = engine
        .commit(PresentationTransactionRequest::mixed(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                node(102),
                rect(0.0, 0.0, 10.0, 10.0),
                rect(10.0, 0.0, 10.0, 10.0),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
            vec![PresentationOpacityMutation::new(
                node(102),
                PresentationOpacity::OPAQUE,
                PresentationOpacity::new(0.5).expect("opacity"),
                AnimationCurve::easing(Duration::from_millis(20), EasingCurve::Linear),
            )],
        ))
        .expect("mixed transaction");
    assert_eq!(transaction.members().len(), 2);
    assert_ne!(
        transaction.members()[0].revision_id(),
        transaction.members()[1].revision_id()
    );
    let output = OutputId::from_raw(1).expect("output");
    let frame = engine.sample(
        output,
        AnimationTime::from_nanos(20_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target(node(102), 102, rect(10.0, 0.0, 10.0, 10.0))],
    );
    let geometry = frame.transforms[0];
    let opacity = frame.opacities[0];
    assert!(engine.acknowledge_presented_geometry(
        output,
        PresentedGeometryAck::from_transform(output, geometry)
    ));
    assert_eq!(engine.active_count(), 1);
    assert_eq!(engine.transaction_count(), 1);
    let opacity_transition = opacity.transition.expect("opacity transition");
    assert!(engine.acknowledge_presented_opacity(
        output,
        opacity_ack(
            output,
            node(102),
            opacity_transition.transaction_id,
            opacity_transition.revision_id,
            0.5,
        )
    ));
    assert_eq!(engine.active_count(), 0);
    assert_eq!(engine.transaction_count(), 0);
}

#[test]
fn geometry_opacity_and_clip_share_one_transaction_and_ack_independently() {
    let mut engine = PresentationEngine::enabled();
    let scene_node_id = node(125);
    let output = OutputId::from_raw(1).expect("output");
    let transaction = engine
        .commit(PresentationTransactionRequest::mixed_all(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                scene_node_id,
                rect(0.0, 0.0, 10.0, 10.0),
                rect(10.0, 0.0, 20.0, 20.0),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
            vec![PresentationOpacityMutation::new(
                scene_node_id,
                PresentationOpacity::OPAQUE,
                PresentationOpacity::new(0.5).expect("opacity"),
                AnimationCurve::easing(Duration::from_millis(20), EasingCurve::Linear),
            )],
            vec![clip_mutation(
                scene_node_id,
                PresentationClip::Unbounded,
                PresentationClip::Rect(clip_rect(2.0, 3.0, 4.0, 5.0)),
                Some(clip_rect(-50.0, -50.0, 120.0, 120.0)),
                AnimationCurve::easing(Duration::from_millis(30), EasingCurve::Linear),
            )],
        ))
        .expect("three-property transaction");
    assert_eq!(transaction.members().len(), 3);
    assert_eq!(
        transaction
            .members()
            .iter()
            .map(|member| member.property())
            .collect::<Vec<_>>(),
        [
            PresentationPropertyKind::Geometry,
            PresentationPropertyKind::Opacity,
            PresentationPropertyKind::Clip,
        ]
    );
    assert_eq!(
        transaction
            .members()
            .iter()
            .map(|member| member.revision_id())
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );

    let frame = engine.sample(
        output,
        AnimationTime::from_nanos(30_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target(scene_node_id, 125, rect(10.0, 0.0, 20.0, 20.0))],
    );
    let geometry = frame.transforms[0];
    let opacity = frame.opacities[0];
    let clip = frame.clips[0];
    assert!(engine.acknowledge_presented_geometry(
        output,
        PresentedGeometryAck::from_transform(output, geometry)
    ));
    assert_eq!(engine.active_count(), 2);
    let opacity_transition = opacity.transition.expect("opacity transition evidence");
    assert!(engine.acknowledge_presented_opacity(
        output,
        opacity_ack(
            output,
            scene_node_id,
            opacity_transition.transaction_id,
            opacity_transition.revision_id,
            0.5,
        )
    ));
    assert_eq!(engine.active_count(), 1);
    assert!(engine.acknowledge_presented_clip(
        output,
        PresentedClipAck::from_group_clip(output, clip).expect("clip transition evidence"),
    ));
    assert_eq!(engine.active_count(), 0);
    assert_eq!(engine.transaction_count(), 0);
}

#[test]
fn clip_identity_noop_and_wrong_output_or_stale_ack_do_not_retire_track() {
    let mut engine = PresentationEngine::enabled();
    let scene_node_id = node(126);
    let output = OutputId::from_raw(1).expect("output");
    let other_output = OutputId::from_raw(2).expect("other output");
    assert_eq!(
        engine.commit(PresentationTransactionRequest::clip(
            AnimationTime::from_nanos(0),
            vec![clip_mutation(
                scene_node_id,
                PresentationClip::Unbounded,
                PresentationClip::Unbounded,
                None,
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        )),
        Err(PresentationTransactionError::Empty)
    );
    let target_clip = PresentationClip::Rect(clip_rect(0.0, 0.0, 10.0, 10.0));
    let transaction = engine
        .commit(PresentationTransactionRequest::clip(
            AnimationTime::from_nanos(0),
            vec![clip_mutation(
                scene_node_id,
                PresentationClip::Unbounded,
                target_clip,
                Some(clip_rect(-100.0, -100.0, 200.0, 200.0)),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("clip transition");
    assert_eq!(transaction.id().get(), 1, "identity no-op allocates no id");
    let frame = engine.sample(
        output,
        AnimationTime::from_nanos(10_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target(scene_node_id, 126, rect(0.0, 0.0, 10.0, 10.0))],
    );
    let group = frame.clips[0];
    let transition = group.transition.expect("transition evidence");
    let wrong_output = clip_ack(
        other_output,
        scene_node_id,
        transition.transaction_id,
        transition.revision_id,
        target_clip,
    );
    assert!(!engine.acknowledge_presented_clip(output, wrong_output));
    let stale = clip_ack(
        output,
        scene_node_id,
        transition.transaction_id,
        PresentationRevisionId::from_raw(999).expect("stale revision"),
        target_clip,
    );
    assert!(!engine.acknowledge_presented_clip(output, stale));
    assert!(engine.has_clip_track(scene_node_id));
    assert!(engine.acknowledge_presented_clip(
        output,
        PresentedClipAck::from_group_clip(output, group).expect("exact physical ack"),
    ));
    assert!(!engine.has_clip_track(scene_node_id));
}

#[test]
fn invalid_opacity_does_not_partially_commit_geometry() {
    let mut engine = PresentationEngine::enabled();
    let result = engine.commit(PresentationTransactionRequest::mixed(
        AnimationTime::from_nanos(0),
        vec![PresentationGeometryMutation::new(
            node(103),
            rect(0.0, 0.0, 10.0, 10.0),
            rect(10.0, 0.0, 10.0, 10.0),
            AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
        )],
        vec![PresentationOpacityMutation::without_owner(
            PresentationOpacity::OPAQUE,
            PresentationOpacity::TRANSPARENT,
            AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
        )],
    ));
    assert_eq!(
        result,
        Err(PresentationTransactionError::MissingPresentationOwner)
    );
    assert_eq!(engine.active_count(), 0);
    assert_eq!(engine.transaction_count(), 0);
}

#[test]
fn settled_unacked_opacity_same_target_preserves_revision() {
    let mut engine = PresentationEngine::enabled();
    let target_opacity = PresentationOpacity::new(0.5).expect("opacity");
    let first = engine
        .commit(PresentationTransactionRequest::opacity(
            AnimationTime::from_nanos(0),
            vec![PresentationOpacityMutation::new(
                node(104),
                PresentationOpacity::OPAQUE,
                target_opacity,
                AnimationCurve::easing(Duration::from_millis(1), EasingCurve::Linear),
            )],
        ))
        .expect("first opacity transaction");
    let second = engine.commit(PresentationTransactionRequest::opacity(
        AnimationTime::from_nanos(2_000_000),
        vec![PresentationOpacityMutation::new(
            node(104),
            PresentationOpacity::OPAQUE,
            target_opacity,
            AnimationCurve::easing(Duration::from_millis(1), EasingCurve::Linear),
        )],
    ));
    assert_eq!(second, Err(PresentationTransactionError::Empty));
    assert_eq!(
        engine.opacity_track_transaction(node(104)),
        Some(first.id())
    );
    assert_eq!(
        engine.opacity_track_revision(node(104)),
        Some(first.members()[0].revision_id())
    );
}

#[test]
fn stale_and_wrong_output_opacity_acks_preserve_the_current_revision() {
    let mut engine = PresentationEngine::enabled();
    let output = OutputId::from_raw(1).expect("output");
    let other_output = OutputId::from_raw(2).expect("other output");
    let first = engine
        .commit(PresentationTransactionRequest::opacity(
            AnimationTime::from_nanos(0),
            vec![PresentationOpacityMutation::new(
                node(108),
                PresentationOpacity::OPAQUE,
                PresentationOpacity::new(0.5).expect("opacity"),
                AnimationCurve::easing(Duration::from_millis(1), EasingCurve::Linear),
            )],
        ))
        .expect("first transaction");
    let old_frame = engine.sample(
        output,
        AnimationTime::from_nanos(1_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target(node(108), 108, rect(0.0, 0.0, 10.0, 10.0))],
    );
    let old_transition = old_frame.opacities[0].transition.expect("old transition");
    assert!(old_transition.mathematically_settled);

    let second = engine
        .commit(PresentationTransactionRequest::opacity(
            AnimationTime::from_nanos(1_000_000),
            vec![PresentationOpacityMutation::new(
                node(108),
                PresentationOpacity::OPAQUE,
                PresentationOpacity::new(0.25).expect("opacity"),
                AnimationCurve::easing(Duration::from_millis(1), EasingCurve::Linear),
            )],
        ))
        .expect("retarget transaction");
    let new_frame = engine.sample(
        output,
        AnimationTime::from_nanos(2_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target(node(108), 108, rect(0.0, 0.0, 10.0, 10.0))],
    );
    let new_transition = new_frame.opacities[0].transition.expect("new transition");
    assert!(new_transition.mathematically_settled);
    assert!(!engine.acknowledge_presented_opacity(
        output,
        opacity_ack(
            output,
            node(108),
            old_transition.transaction_id,
            old_transition.revision_id,
            0.5,
        ),
    ));
    assert!(!engine.acknowledge_presented_opacity(
        output,
        opacity_ack(
            other_output,
            node(108),
            new_transition.transaction_id,
            new_transition.revision_id,
            0.25,
        ),
    ));
    assert!(engine.acknowledge_presented_opacity(
        output,
        opacity_ack(
            output,
            node(108),
            second.id(),
            new_transition.revision_id,
            0.25,
        ),
    ));
    assert_eq!(engine.transaction_count(), 0);
    assert_eq!(first.members().len(), 1);
}

#[test]
fn opacity_spring_visible_sample_is_bounded_on_both_sides() {
    let mut engine = PresentationEngine::enabled();
    let spring = AnimationCurve::spring(SpringSpec::new(100.0, 1.0));
    engine
        .commit(PresentationTransactionRequest::opacity(
            AnimationTime::from_nanos(0),
            vec![PresentationOpacityMutation::new(
                node(105),
                PresentationOpacity::TRANSPARENT,
                PresentationOpacity::OPAQUE,
                spring,
            )],
        ))
        .expect("upper overshoot track");
    engine
        .commit(PresentationTransactionRequest::opacity(
            AnimationTime::from_nanos(100_000_000),
            vec![PresentationOpacityMutation::new(
                node(106),
                PresentationOpacity::OPAQUE,
                PresentationOpacity::TRANSPARENT,
                spring,
            )],
        ))
        .expect("lower overshoot track");

    for node_id in [node(105), node(106)] {
        let (opacity, velocity, _) = engine
            .sample_opacity_for_scene_node(node_id, AnimationTime::from_nanos(300_000_000))
            .expect("opacity sample");
        assert!((0.0..=1.0).contains(&opacity.get()));
        if opacity.is_transparent() || opacity.is_opaque() {
            assert_eq!(velocity, 0.0);
        }
    }
}

#[test]
fn geometry_transaction_assigns_one_transaction_and_distinct_revisions() {
    let mut engine = PresentationEngine::enabled();
    let request = PresentationTransactionRequest::geometry(
        AnimationTime::from_nanos(0),
        vec![
            PresentationGeometryMutation::new(
                node(11),
                rect(0.0, 0.0, 100.0, 100.0),
                rect(100.0, 0.0, 100.0, 100.0),
                AnimationCurve::easing(Duration::from_millis(100), EasingCurve::Linear),
            ),
            PresentationGeometryMutation::new(
                node(12),
                rect(0.0, 100.0, 100.0, 100.0),
                rect(100.0, 100.0, 100.0, 100.0),
                AnimationCurve::easing(Duration::from_millis(100), EasingCurve::Linear),
            ),
        ],
    );
    let committed = engine.commit(request).expect("transaction commits");

    assert_eq!(committed.members().len(), 2);
    assert_eq!(committed.members()[0].transaction_id(), committed.id());
    assert_eq!(
        committed.members()[0].transaction_id(),
        committed.members()[1].transaction_id()
    );
    assert_ne!(
        committed.members()[0].revision_id(),
        committed.members()[1].revision_id()
    );
    assert_eq!(engine.active_count(), 2);
    assert_eq!(
        engine
            .sample(
                OutputId::from_raw(1).expect("output"),
                AnimationTime::from_nanos(50_000_000),
                PresentationSampleTimeSource::ScheduledTarget,
                &[
                    target(node(11), 11, rect(100.0, 0.0, 100.0, 100.0)),
                    target(node(12), 12, rect(100.0, 100.0, 100.0, 100.0)),
                ],
            )
            .transforms
            .len(),
        2
    );
}

#[test]
fn duplicate_member_rejection_leaves_existing_state_unchanged() {
    let mut engine = PresentationEngine::enabled();
    let first = engine
        .commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(10),
            vec![PresentationGeometryMutation::new(
                node(21),
                rect(0.0, 0.0, 10.0, 10.0),
                rect(20.0, 0.0, 10.0, 10.0),
                AnimationCurve::easing(Duration::from_millis(100), EasingCurve::EaseOut),
            )],
        ))
        .expect("initial transaction");
    let result = engine.commit(PresentationTransactionRequest::geometry(
        AnimationTime::from_nanos(20),
        vec![
            PresentationGeometryMutation::new(
                node(21),
                rect(0.0, 0.0, 10.0, 10.0),
                rect(30.0, 0.0, 10.0, 10.0),
                AnimationCurve::easing(Duration::from_millis(100), EasingCurve::Linear),
            ),
            PresentationGeometryMutation::new(
                node(21),
                rect(0.0, 0.0, 10.0, 10.0),
                rect(40.0, 0.0, 10.0, 10.0),
                AnimationCurve::easing(Duration::from_millis(100), EasingCurve::Linear),
            ),
        ],
    ));
    assert_eq!(result, Err(PresentationTransactionError::DuplicateProperty));
    assert_eq!(engine.active_count(), 1);
    assert_eq!(engine.transaction_count(), 1);
    assert_eq!(engine.track_transaction(node(21)), Some(first.id()));
    assert_eq!(
        engine.track_revision(node(21)),
        Some(first.members()[0].revision_id())
    );
}

#[test]
fn missing_owner_and_invalid_geometry_are_rejected_without_mutation() {
    let mut engine = PresentationEngine::enabled();
    let start = rect(0.0, 0.0, 10.0, 10.0);
    let target_rect = rect(20.0, 0.0, 10.0, 10.0);
    assert_eq!(
        engine.commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::without_owner(
                start,
                target_rect,
                AnimationCurve::easing(Duration::from_millis(100), EasingCurve::Linear),
            )],
        )),
        Err(PresentationTransactionError::MissingPresentationOwner)
    );
    let invalid = PresentationRect::from_raw_for_test(f64::NAN, 0.0, 10.0, 10.0);
    assert_eq!(
        engine.commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                node(22),
                invalid,
                target_rect,
                AnimationCurve::easing(Duration::from_millis(100), EasingCurve::Linear),
            )],
        )),
        Err(PresentationTransactionError::InvalidGeometry)
    );
    assert_eq!(engine.active_count(), 0);
    assert_eq!(engine.transaction_count(), 0);
}

#[test]
fn retarget_preserves_position_and_velocity_and_uses_new_curve() {
    let mut engine = PresentationEngine::enabled();
    let spring = AnimationCurve::spring(SpringSpec::new(100.0, 16.0));
    engine
        .commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                node(31),
                rect(0.0, 0.0, 10.0, 10.0),
                rect(100.0, 0.0, 10.0, 10.0),
                spring,
            )],
        ))
        .expect("initial track");
    let at = AnimationTime::from_nanos(120_000_000);
    let before = engine
        .sample_compat(31, at)
        .expect("active sample before retarget");
    let new_curve = AnimationCurve::easing(Duration::from_millis(200), EasingCurve::EaseIn);
    let retargeted = engine
        .commit(PresentationTransactionRequest::geometry(
            at,
            vec![PresentationGeometryMutation::new(
                node(31),
                rect(-100.0, 0.0, 10.0, 10.0),
                rect(20.0, 40.0, 10.0, 10.0),
                new_curve,
            )],
        ))
        .expect("retarget transaction");
    let after = engine
        .sample_compat(31, at)
        .expect("active sample after retarget");
    assert_eq!(after.rect, before.rect);
    assert_eq!(after.velocity, before.velocity);
    assert_eq!(engine.track_curve(node(31)), Some(new_curve));
    assert_ne!(retargeted.members()[0].revision_id(), before.revision_id);
}

#[test]
fn settled_track_requires_matching_physical_revision_ack() {
    let mut engine = PresentationEngine::enabled();
    let transaction = engine
        .commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                node(41),
                rect(0.0, 0.0, 10.0, 10.0),
                rect(20.0, 0.0, 10.0, 10.0),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("transaction");
    let output = OutputId::from_raw(1).expect("output");
    let revision = transaction.members()[0].revision_id();
    let frame = engine.sample(
        output,
        AnimationTime::from_nanos(20_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target(node(41), 41, rect(20.0, 0.0, 10.0, 10.0))],
    );
    assert!(frame.transforms[0].mathematically_settled);
    assert_eq!(engine.active_count(), 1);
    assert!(!engine.acknowledge_presented_geometry(
        output,
        geometry_ack(
            OutputId::from_raw(2).expect("wrong output"),
            node(41),
            transaction.id(),
            revision,
            rect(20.0, 0.0, 10.0, 10.0),
        ),
    ));
    assert_eq!(engine.active_count(), 1);
    assert!(!engine.acknowledge_presented_geometry(
        output,
        geometry_ack(
            output,
            node(42),
            transaction.id(),
            revision,
            rect(20.0, 0.0, 10.0, 10.0),
        ),
    ));
    assert_eq!(engine.active_count(), 1);
    assert!(engine.acknowledge_presented_geometry(
        output,
        geometry_ack(
            output,
            node(41),
            transaction.id(),
            revision,
            rect(20.0, 0.0, 10.0, 10.0),
        ),
    ));
    assert_eq!(engine.active_count(), 0);
    assert_eq!(engine.transaction_count(), 0);
}

#[test]
fn frozen_output_a_ack_survives_sampling_output_b() {
    let mut engine = PresentationEngine::enabled();
    let transaction = engine
        .commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                node(45),
                rect(0.0, 0.0, 10.0, 10.0),
                rect(20.0, 0.0, 10.0, 10.0),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("transaction");
    let output_a = OutputId::from_raw(1).expect("output A");
    let output_b = OutputId::from_raw(2).expect("output B");
    let frame_a = engine.sample(
        output_a,
        AnimationTime::from_nanos(20_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target(node(45), 45, rect(20.0, 0.0, 10.0, 10.0))],
    );
    let _frame_b = engine.sample(
        output_b,
        AnimationTime::from_nanos(20_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target(node(45), 45, rect(20.0, 0.0, 10.0, 10.0))],
    );
    let transform = frame_a.transforms[0];
    let ack = PresentedGeometryAck {
        output_id: frame_a.output_id,
        scene_node_id: transform.scene_node_id,
        property: PresentationPropertyKind::Geometry,
        transaction_id: transform.transaction_id,
        revision_id: transform.revision_id,
        presented_rect: transform.presented_rect,
    };

    assert_eq!(ack.transaction_id, transaction.id());
    assert!(engine.acknowledge_presented_geometry(output_a, ack));
    assert_eq!(engine.active_count(), 0);
}

#[test]
fn wrong_output_ack_is_rejected_without_mutating_the_active_track() {
    let mut engine = PresentationEngine::enabled();
    let transaction = engine
        .commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                node(46),
                rect(0.0, 0.0, 10.0, 10.0),
                rect(20.0, 0.0, 10.0, 10.0),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("transaction");
    let output_a = OutputId::from_raw(1).expect("output A");
    let output_b = OutputId::from_raw(2).expect("output B");
    let frame_a = engine.sample(
        output_a,
        AnimationTime::from_nanos(20_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target(node(46), 46, rect(20.0, 0.0, 10.0, 10.0))],
    );
    let _frame_b = engine.sample(
        output_b,
        AnimationTime::from_nanos(20_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target(node(46), 46, rect(20.0, 0.0, 10.0, 10.0))],
    );
    let transform = frame_a.transforms[0];
    let mut wrong_output_ack = PresentedGeometryAck {
        output_id: output_b,
        scene_node_id: transform.scene_node_id,
        property: PresentationPropertyKind::Geometry,
        transaction_id: transform.transaction_id,
        revision_id: transform.revision_id,
        presented_rect: transform.presented_rect,
    };
    let before = engine.metrics();

    assert!(!engine.acknowledge_presented_geometry(output_a, wrong_output_ack));
    assert_eq!(engine.active_count(), 1);
    assert_eq!(engine.transaction_count(), 1);
    assert_eq!(engine.track_transaction(node(46)), Some(transaction.id()));
    assert_eq!(engine.track_revision(node(46)), Some(transform.revision_id));
    assert_eq!(
        engine.metrics().wrong_output_acks,
        before.wrong_output_acks + 1
    );

    wrong_output_ack.output_id = output_a;
    assert!(engine.acknowledge_presented_geometry(output_a, wrong_output_ack));
    assert_eq!(engine.active_count(), 0);
}

#[test]
fn transaction_member_retirement_requires_exact_revision_evidence() {
    let transaction_id = PresentationTransactionId::new(NonZeroU64::new(7).expect("transaction"));
    let current_revision = PresentationRevisionId::from_raw(10).expect("current revision");
    let stale_revision = PresentationRevisionId::from_raw(9).expect("stale revision");
    let final_revision = PresentationRevisionId::from_raw(11).expect("final revision");
    let mut record = PresentationTransactionRecord::new(
        transaction_id,
        AnimationTime::from_nanos(0),
        vec![
            PresentationTransactionMember::new(
                node(47),
                PresentationPropertyKind::Geometry,
                transaction_id,
                current_revision,
            ),
            PresentationTransactionMember::new(
                node(48),
                PresentationPropertyKind::Geometry,
                transaction_id,
                final_revision,
            ),
        ],
    );

    assert!(!record.remove_member_exact(
        node(47),
        PresentationPropertyKind::Geometry,
        stale_revision,
    ));
    assert_eq!(record.members().len(), 2);
    assert!(record.remove_member_exact(
        node(47),
        PresentationPropertyKind::Geometry,
        current_revision,
    ));
    assert_eq!(record.members().len(), 1);
    assert!(record.remove_member_exact(
        node(48),
        PresentationPropertyKind::Geometry,
        final_revision,
    ));
    assert!(record.members().is_empty());
}

#[test]
fn stale_revision_ack_cannot_retire_newer_track() {
    let mut engine = PresentationEngine::enabled();
    let initial = engine
        .commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                node(51),
                rect(0.0, 0.0, 10.0, 10.0),
                rect(20.0, 0.0, 10.0, 10.0),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("initial transaction");
    let old_revision = initial.members()[0].revision_id();
    let newer = engine
        .commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(5_000_000),
            vec![PresentationGeometryMutation::new(
                node(51),
                rect(0.0, 0.0, 10.0, 10.0),
                rect(40.0, 0.0, 10.0, 10.0),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("retarget transaction");
    let output = OutputId::from_raw(1).expect("output");
    assert!(!engine.acknowledge_presented_geometry(
        output,
        geometry_ack(
            output,
            node(51),
            initial.id(),
            old_revision,
            rect(20.0, 0.0, 10.0, 10.0),
        ),
    ));
    assert_eq!(
        engine.track_revision(node(51)),
        Some(newer.members()[0].revision_id())
    );
}

#[test]
fn backing_surface_replacement_keeps_window_group_revision_and_history_identity() {
    let mut engine = PresentationEngine::enabled();
    let group = node(81);
    let transaction = engine
        .commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                group,
                rect(0.0, 0.0, 100.0, 80.0),
                rect(100.0, 20.0, 120.0, 96.0),
                AnimationCurve::spring(SpringSpec::new(100.0, 16.0)),
            )],
        ))
        .expect("geometry transaction");
    let output = OutputId::from_raw(1).expect("output");
    let old_frame = engine.sample(
        output,
        AnimationTime::from_nanos(50_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target(group, 901, rect(100.0, 20.0, 120.0, 96.0))],
    );
    let new_frame = engine.sample(
        output,
        AnimationTime::from_nanos(60_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target(group, 902, rect(100.0, 20.0, 120.0, 96.0))],
    );
    assert_eq!(old_frame.transforms[0].scene_node_id, group);
    assert_eq!(new_frame.transforms[0].scene_node_id, group);
    assert_eq!(
        old_frame.transforms[0].revision_id,
        transaction.members()[0].revision_id()
    );
    assert_eq!(
        new_frame.transforms[0].revision_id,
        transaction.members()[0].revision_id()
    );
    assert_eq!(
        old_frame.transforms[0].transaction_id,
        new_frame.transforms[0].transaction_id
    );
    assert_ne!(
        old_frame.transforms[0].root_surface_id,
        new_frame.transforms[0].root_surface_id
    );
    assert!(old_frame.windows[0].velocity != new_frame.windows[0].velocity);

    let old_snapshot = old_frame.frame_snapshot();
    assert_eq!(old_snapshot.transforms[0].root_surface_id, 901);
    assert_eq!(old_snapshot.transforms[0].scene_node_id, group);
}

#[test]
fn id_exhaustion_is_checked_before_commit_mutation() {
    let mut engine = PresentationEngine::enabled();
    engine.set_next_ids_for_test(NonZeroU64::MAX, NonZeroU64::MAX);
    let request = || {
        PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                node(61),
                rect(0.0, 0.0, 10.0, 10.0),
                rect(20.0, 0.0, 10.0, 10.0),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        )
    };
    let first = engine
        .commit(request())
        .expect("last IDs can be allocated once");
    assert_eq!(first.id().get(), u64::MAX);
    assert_eq!(engine.active_count(), 1);
    let second = engine.commit(request());
    assert_eq!(
        second,
        Err(PresentationTransactionError::TransactionIdExhausted)
    );
    assert_eq!(
        engine.track_revision(node(61)),
        Some(first.members()[0].revision_id())
    );
}

#[test]
fn revision_id_exhaustion_is_checked_before_commit_mutation() {
    let mut engine = PresentationEngine::enabled();
    engine.set_next_ids_for_test(NonZeroU64::new(7).expect("transaction id"), NonZeroU64::MAX);
    let request = || {
        PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                node(62),
                rect(0.0, 0.0, 10.0, 10.0),
                rect(20.0, 0.0, 10.0, 10.0),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        )
    };
    let first = engine
        .commit(request())
        .expect("last revision can be allocated once");
    assert_eq!(first.members()[0].revision_id().get(), u64::MAX);
    let second = engine.commit(request());
    assert_eq!(
        second,
        Err(PresentationTransactionError::RevisionIdExhausted)
    );
    assert_eq!(engine.active_count(), 1);
    assert_eq!(engine.transaction_count(), 1);
    assert_eq!(
        engine.track_revision(node(62)),
        Some(first.members()[0].revision_id())
    );
}

#[test]
fn empty_presentation_samples_and_snapshots_preserve_explicit_output_identity() {
    let output_a = OutputId::from_raw(1).expect("output A");
    let output_b = OutputId::from_raw(2).expect("output B");
    let sampled_at = AnimationTime::from_nanos(17);

    let sample_a = PresentationSceneSample::empty_for_output(
        output_a,
        sampled_at,
        PresentationSampleTimeSource::ZeroFallback,
    );
    let sample_b = PresentationSceneSample::empty_for_output(
        output_b,
        sampled_at,
        PresentationSampleTimeSource::ZeroFallback,
    );

    assert_eq!(sample_a.output_id, output_a);
    assert_eq!(sample_b.output_id, output_b);
    assert_eq!(
        PresentationFrameSnapshot::empty_for_output(output_a).output_id,
        output_a
    );
    assert_eq!(
        PresentationFrameSnapshot::empty_for_output(output_b).output_id,
        output_b
    );
}

#[test]
fn inactive_identity_geometry_is_empty_without_allocating_presentation_work() {
    let mut engine = PresentationEngine::enabled();
    let scene_node_id = node(101);
    let identity = rect(10.0, 20.0, 30.0, 40.0);
    let request = || {
        PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                scene_node_id,
                identity,
                identity,
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        )
    };
    let before = engine.metrics();

    assert_eq!(
        engine.commit(request()),
        Err(PresentationTransactionError::Empty)
    );
    assert_eq!(engine.metrics(), before);
    assert_eq!(engine.active_count(), 0);
    assert_eq!(engine.transaction_count(), 0);
    assert!(!engine.has_pending_visible(&[scene_node_id]));

    engine.set_next_ids_for_test(NonZeroU64::MAX, NonZeroU64::MAX);
    assert_eq!(
        engine.commit(request()),
        Err(PresentationTransactionError::Empty)
    );
    assert_eq!(engine.metrics(), before);
    engine.set_next_ids_for_test(
        NonZeroU64::new(41).expect("transaction id"),
        NonZeroU64::new(51).expect("revision id"),
    );
    let effective = engine
        .commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                scene_node_id,
                identity,
                rect(11.0, 20.0, 30.0, 40.0),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("effective transaction");
    assert_eq!(effective.id().get(), 41);
    assert_eq!(effective.members()[0].revision_id().get(), 51);
}

#[test]
fn mixed_geometry_transaction_contains_only_effective_members() {
    let mut engine = PresentationEngine::enabled();
    let identity_a = rect(0.0, 0.0, 10.0, 10.0);
    let identity_c = rect(20.0, 20.0, 10.0, 10.0);
    let record = engine
        .commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![
                PresentationGeometryMutation::new(
                    node(111),
                    identity_a,
                    identity_a,
                    AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
                ),
                PresentationGeometryMutation::new(
                    node(112),
                    rect(0.0, 20.0, 10.0, 10.0),
                    rect(20.0, 20.0, 10.0, 10.0),
                    AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
                ),
                PresentationGeometryMutation::new(
                    node(113),
                    identity_c,
                    identity_c,
                    AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
                ),
            ],
        ))
        .expect("one effective member commits");

    assert_eq!(record.members().len(), 1);
    assert_eq!(record.members()[0].scene_node_id(), node(112));
    assert!(!engine.has_track(node(111)));
    assert!(engine.has_track(node(112)));
    assert!(!engine.has_track(node(113)));
}

#[test]
fn moving_active_track_retargets_when_canonical_start_equals_target() {
    let mut engine = PresentationEngine::enabled();
    let scene_node_id = node(121);
    let start = rect(0.0, 0.0, 10.0, 10.0);
    let target_rect = rect(100.0, 0.0, 10.0, 10.0);
    let spring = AnimationCurve::spring(SpringSpec::new(100.0, 16.0));
    let initial = engine
        .commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                scene_node_id,
                start,
                target_rect,
                spring,
            )],
        ))
        .expect("initial track");
    let retarget_at = AnimationTime::from_nanos(50_000_000);
    let sampled = engine
        .sample_for_scene_node(scene_node_id, retarget_at)
        .expect("active sample");
    assert!(!sampled.mathematically_settled);

    let retargeted = engine
        .commit(PresentationTransactionRequest::geometry(
            retarget_at,
            vec![PresentationGeometryMutation::new(
                scene_node_id,
                target_rect,
                target_rect,
                AnimationCurve::easing(Duration::from_millis(100), EasingCurve::EaseOut),
            )],
        ))
        .expect("moving active track remains effective");
    assert_ne!(
        retargeted.members()[0].revision_id(),
        initial.members()[0].revision_id()
    );
    let retargeted_sample = engine
        .sample_for_scene_node(scene_node_id, retarget_at)
        .expect("retargeted sample");
    assert_eq!(retargeted_sample.rect, sampled.rect);
    assert_eq!(retargeted_sample.velocity, sampled.velocity);
    assert_eq!(retargeted_sample.rect, sampled.rect);
    assert_eq!(
        engine.track_curve(scene_node_id),
        Some(AnimationCurve::easing(
            Duration::from_millis(100),
            EasingCurve::EaseOut,
        ))
    );
}

#[test]
fn settled_unacknowledged_same_target_keeps_existing_revision_until_ack() {
    let mut engine = PresentationEngine::enabled();
    let output_id = OutputId::from_raw(1).expect("output");
    let scene_node_id = node(131);
    let target_rect = rect(20.0, 0.0, 10.0, 10.0);
    let initial = engine
        .commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                scene_node_id,
                rect(0.0, 0.0, 10.0, 10.0),
                target_rect,
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("initial track");
    let settled_at = AnimationTime::from_nanos(20_000_000);
    let sampled = engine
        .sample_for_scene_node(scene_node_id, settled_at)
        .expect("settled sample");
    assert!(sampled.mathematically_settled);
    let before = engine.metrics();

    assert_eq!(
        engine.commit(PresentationTransactionRequest::geometry(
            settled_at,
            vec![PresentationGeometryMutation::new(
                scene_node_id,
                target_rect,
                target_rect,
                AnimationCurve::easing(Duration::from_millis(100), EasingCurve::EaseOut),
            )],
        )),
        Err(PresentationTransactionError::Empty)
    );
    assert_eq!(engine.metrics(), before);
    assert_eq!(engine.track_transaction(scene_node_id), Some(initial.id()));
    assert_eq!(
        engine.track_revision(scene_node_id),
        Some(initial.members()[0].revision_id())
    );
    assert_eq!(engine.transaction_count(), 1);
    assert!(engine.acknowledge_presented_geometry(
        output_id,
        geometry_ack(
            output_id,
            scene_node_id,
            initial.id(),
            initial.members()[0].revision_id(),
            target_rect,
        ),
    ));
    assert_eq!(engine.active_count(), 0);
    assert_eq!(engine.transaction_count(), 0);
}
