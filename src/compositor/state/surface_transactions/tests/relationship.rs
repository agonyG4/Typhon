use super::*;
use crate::compositor::subsurface::*;

#[test]
fn relationship_activation_follows_pending_latched_applied_lifecycle() {
    let mut transactions = SurfaceTransactionState::default();

    assert!(transactions.register(2, 1));
    assert_eq!(
        transactions.relationship_phase(2),
        Some(SubsurfaceRelationshipPhase::PendingParentCommit)
    );
    assert!(transactions.relationship_is_registered_child_of(2, 1));
    assert!(!transactions.relationship_is_applied_child_of(2, 1));

    let captured = transactions.captured_relationship(2).unwrap();
    assert_eq!(
        transactions.take_pending_relationship_activations_for_parent(1),
        vec![captured]
    );
    assert_eq!(
        transactions.relationship_phase(2),
        Some(SubsurfaceRelationshipPhase::Latched)
    );
    assert!(
        transactions
            .take_pending_relationship_activations_for_parent(1)
            .is_empty()
    );

    assert!(transactions.apply_captured_relationship(captured));
    assert_eq!(
        transactions.relationship_phase(2),
        Some(SubsurfaceRelationshipPhase::Applied)
    );
    assert!(transactions.relationship_is_applied_child_of(2, 1));
}

#[test]
fn relationship_id_allocation_fails_without_wrapping() {
    let mut transactions = SurfaceTransactionState {
        next_relationship_id: u64::MAX,
        ..SurfaceTransactionState::default()
    };

    assert!(!transactions.register(2, 1));
    assert_eq!(transactions.relationship_phase(2), None);
}

#[test]
fn nested_relationship_activations_are_captured_at_each_parent_boundary() {
    let mut transactions = SurfaceTransactionState::default();
    assert!(transactions.register(2, 1));
    assert!(transactions.register(3, 2));

    let child = transactions.captured_relationship(2).unwrap();
    let grandchild = transactions.captured_relationship(3).unwrap();
    assert_eq!(
        transactions.take_pending_relationship_activations_for_parent(1),
        vec![child]
    );
    assert_eq!(
        transactions.take_pending_relationship_activations_for_parent(2),
        vec![grandchild]
    );
}

#[test]
fn destroying_each_relationship_phase_removes_its_live_identity() {
    let mut transactions = SurfaceTransactionState::default();

    assert!(transactions.register(2, 1));
    assert!(transactions.remove_role(2).is_empty());
    assert_eq!(transactions.relationship_phase(2), None);

    assert!(transactions.register(3, 1));
    let captured = transactions.captured_relationship(3).unwrap();
    assert_eq!(
        transactions.take_pending_relationship_activations_for_parent(1),
        vec![captured]
    );
    assert!(transactions.remove_role(3).is_empty());
    assert_eq!(transactions.relationship_phase(3), None);
    assert!(!transactions.relationship_matches(captured));

    assert!(transactions.register(4, 1));
    let captured = transactions.captured_relationship(4).unwrap();
    assert_eq!(
        transactions.take_pending_relationship_activations_for_parent(1),
        vec![captured]
    );
    assert!(transactions.apply_captured_relationship(captured));
    assert!(transactions.remove_role(4).is_empty());
    assert_eq!(transactions.relationship_phase(4), None);
}

#[test]
fn relationship_ids_are_monotonic_and_stale_captures_are_rejected() {
    let mut transactions = SurfaceTransactionState::default();

    assert!(transactions.register(2, 1));
    let first = transactions.captured_relationship(2).unwrap();
    assert_eq!(
        transactions.take_pending_relationship_activations_for_parent(1),
        vec![first]
    );
    assert!(transactions.remove_role(2).is_empty());

    assert!(transactions.register(2, 1));
    let second = transactions.captured_relationship(2).unwrap();
    assert_ne!(first.relationship_id, second.relationship_id);
    assert!(!transactions.apply_captured_relationship(first));
    assert_eq!(
        transactions.relationship_phase(2),
        Some(SubsurfaceRelationshipPhase::PendingParentCommit)
    );
    assert_eq!(
        transactions.take_pending_relationship_activations_for_parent(1),
        vec![second]
    );
    assert!(transactions.apply_captured_relationship(second));
}
