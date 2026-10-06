use super::surface_transactions::SurfaceTransactionState;

#[test]
fn one_owner_exposes_both_cached_and_materialized_transaction_state() {
    let transactions = SurfaceTransactionState::default();

    assert_eq!(transactions.cached_entry_count(), 0);
    assert_eq!(transactions.pending_tree_count(), 0);
    assert!(!transactions.has_pending_trees());
}
