use super::*;
use crate::core::{OutputId, SceneNodeId};
use std::num::NonZeroU64;
use std::time::Duration;

fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
    PresentationRect::new(x, y, width, height).expect("valid presentation rect")
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
        OutputId::from_raw(2).expect("wrong output"),
        node(41),
        revision,
        rect(20.0, 0.0, 10.0, 10.0),
        Some(transaction.id()),
    ));
    assert_eq!(engine.active_count(), 1);
    assert!(!engine.acknowledge_presented_geometry(
        output,
        node(42),
        revision,
        rect(20.0, 0.0, 10.0, 10.0),
        Some(transaction.id()),
    ));
    assert_eq!(engine.active_count(), 1);
    assert!(engine.acknowledge_presented_geometry(
        output,
        node(41),
        revision,
        rect(20.0, 0.0, 10.0, 10.0),
        Some(transaction.id()),
    ));
    assert_eq!(engine.active_count(), 0);
    assert_eq!(engine.transaction_count(), 0);
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
        node(51),
        old_revision,
        rect(20.0, 0.0, 10.0, 10.0),
        Some(initial.id()),
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
