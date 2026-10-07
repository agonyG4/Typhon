use super::*;

mod lineage;
mod model;
mod queue;
mod settlement;
mod state;

pub use model::SurfaceTreeTransactionId;
pub(in crate::compositor) use model::{
    ActiveSurfacePresentationCommit, BufferlessSurfaceCommitState, PendingSurfaceTreeTransaction,
    ReleasedSurfaceTreeState, SurfacePublicationContext, SurfacePublicationDecision,
    SurfacePublicationSource, SurfacePublicationState, SurfaceTreeAcquireDependency,
    SurfaceTreeMergeStats, SurfaceTreeNodeLifetime, SurfaceTreeNodeLifetimes,
    SurfaceTreeSubmissionKind, TransactionOrdering,
};
pub(in crate::compositor) use state::SurfaceTransactionState;

pub(in crate::compositor) use lineage::{
    add_unique_content_update_ref, content_update_node_covers_ref,
    debug_assert_surface_tree_content_update_invariants, node_index_covering_content_update_ref,
    transaction_covers_content_update_ref, transaction_node_index_covering_content_update_ref,
    transaction_references_any_content_update,
};
#[cfg(test)]
pub(in crate::compositor) use queue::can_coalesce_pending_surface_tree_transaction;
pub(in crate::compositor) use queue::{
    MAX_SURFACE_TREE_TRANSACTIONS_PER_ROOT, normalize_external_content_dependencies_for_nodes,
};
