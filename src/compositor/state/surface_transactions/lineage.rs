use super::*;
use crate::compositor::subsurface::ContentUpdateRef;

pub(in crate::compositor) fn content_update_node_covers_ref(
    node_surface_id: u32,
    commit: &CachedSubsurfaceCommit,
    reference: ContentUpdateRef,
) -> bool {
    if let Some(predecessor) = commit.lineage.predecessor {
        debug_assert_eq!(predecessor.surface_id, node_surface_id);
        debug_assert!(predecessor.commit_sequence < commit.commit_sequence);
    }
    if reference.surface_id != node_surface_id || reference.commit_sequence > commit.commit_sequence
    {
        return false;
    }
    if let Some(predecessor) = commit.lineage.predecessor
        && reference.commit_sequence <= predecessor.commit_sequence
    {
        return false;
    }
    if reference.commit_sequence == commit.commit_sequence {
        debug_assert_eq!(reference.commit_id, commit.commit_id);
        return reference.commit_id == commit.commit_id;
    }
    true
}

pub(in crate::compositor) fn node_index_covering_content_update_ref(
    nodes: &[(u32, CachedSubsurfaceCommit)],
    reference: ContentUpdateRef,
) -> Option<usize> {
    let mut covering_index = None;
    for (index, (surface_id, commit)) in nodes.iter().enumerate() {
        if !content_update_node_covers_ref(*surface_id, commit, reference) {
            continue;
        }
        debug_assert!(
            covering_index.is_none(),
            "overlapping represented Content Update ranges"
        );
        covering_index.get_or_insert(index);
    }
    covering_index
}

pub(in crate::compositor) fn transaction_node_index_covering_content_update_ref(
    transaction: &PendingSurfaceTreeTransaction,
    reference: ContentUpdateRef,
) -> Option<usize> {
    node_index_covering_content_update_ref(&transaction.nodes, reference)
}

pub(in crate::compositor) fn transaction_covers_content_update_ref(
    transaction: &PendingSurfaceTreeTransaction,
    reference: ContentUpdateRef,
) -> bool {
    transaction_node_index_covering_content_update_ref(transaction, reference).is_some()
}

pub(in crate::compositor) fn transaction_references_any_content_update(
    transaction: &PendingSurfaceTreeTransaction,
    references: &[ContentUpdateRef],
) -> bool {
    let references_node = |commit: &CachedSubsurfaceCommit| {
        commit
            .lineage
            .predecessor
            .is_some_and(|predecessor| references.contains(&predecessor))
            || commit
                .lineage
                .child_dependencies
                .iter()
                .any(|dependency| references.contains(dependency))
    };
    transaction
        .external_content_update_dependencies
        .iter()
        .any(|dependency| references.contains(dependency))
        || transaction
            .nodes
            .iter()
            .any(|(_, commit)| references_node(commit))
}

pub(in crate::compositor) fn add_unique_content_update_ref(
    references: &mut Vec<ContentUpdateRef>,
    reference: ContentUpdateRef,
) -> bool {
    if references.contains(&reference) {
        return false;
    }
    references.push(reference);
    true
}

#[cfg(any(debug_assertions, test))]
pub(in crate::compositor) fn debug_assert_surface_tree_content_update_invariants(
    transaction: &PendingSurfaceTreeTransaction,
) {
    let lifetimes = transaction
        .publication_lifetimes
        .captured()
        .expect("surface-tree transactions use captured publication lifetimes");
    debug_assert_eq!(lifetimes.len(), transaction.nodes.len());
    for (node_index, (surface_id, commit)) in transaction.nodes.iter().enumerate() {
        debug_assert_eq!(lifetimes[node_index].surface_id, *surface_id);
        if let Some(predecessor) = commit.lineage.predecessor {
            debug_assert_eq!(predecessor.surface_id, *surface_id);
            debug_assert!(predecessor.commit_sequence < commit.commit_sequence);
        }
        for dependency in &commit.lineage.child_dependencies {
            debug_assert!(dependency.commit_sequence < commit.commit_sequence);
            if let Some(dependency_index) =
                transaction_node_index_covering_content_update_ref(transaction, *dependency)
            {
                debug_assert!(dependency_index <= node_index);
            }
        }
    }
    for (first_index, (first_surface_id, first)) in transaction.nodes.iter().enumerate() {
        for (later_surface_id, later) in transaction.nodes.iter().skip(first_index + 1) {
            if first_surface_id != later_surface_id {
                continue;
            }
            if transaction.ordering() == TransactionOrdering::Coalescible {
                debug_assert!(
                    false,
                    "coalescible transaction retains multiple ranges for one surface"
                );
            }
            debug_assert!(first.commit_sequence < later.commit_sequence);
            debug_assert!(!content_update_node_covers_ref(
                *later_surface_id,
                later,
                first.content_update_ref(*first_surface_id),
            ));
            debug_assert!(!content_update_node_covers_ref(
                *first_surface_id,
                first,
                later.content_update_ref(*later_surface_id),
            ));
        }
    }
    debug_assert!(
        transaction
            .external_content_update_dependencies
            .iter()
            .all(|dependency| !transaction_covers_content_update_ref(transaction, *dependency))
    );
}

#[cfg(not(any(debug_assertions, test)))]
#[inline]
pub(in crate::compositor) fn debug_assert_surface_tree_content_update_invariants(
    _transaction: &PendingSurfaceTreeTransaction,
) {
}
