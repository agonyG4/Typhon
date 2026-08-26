use super::toplevel_publication::{AstreaToplevelCollection, AstreaToplevelPublisher};

impl AstreaToplevelPublisher {
    pub(in crate::compositor) fn should_reconcile(&mut self) -> bool {
        self.metrics.publication_gate_checks =
            self.metrics.publication_gate_checks.saturating_add(1);
        if self.has_pending_publication() {
            true
        } else {
            self.metrics.publication_clean_gate_skips =
                self.metrics.publication_clean_gate_skips.saturating_add(1);
            false
        }
    }

    pub(in crate::compositor) fn has_pending_publication(&self) -> bool {
        self.transaction.is_some()
            || self.initial_reconciliation_pending
            || self.structure_dirty
            || self.next_structure_dirty
            || !self.dirty_windows.is_empty()
            || !self.removed_windows.is_empty()
            || !self.next_dirty_snapshots.is_empty()
            || self.next_collection.is_some()
    }

    pub(in crate::compositor) fn has_active_transaction(&self) -> bool {
        self.transaction.is_some()
    }

    pub(in crate::compositor) fn clear_failed_collection_state(&mut self) {
        self.initial_reconciliation_pending = false;
        self.structure_dirty = false;
        self.next_structure_dirty = false;
        self.next_collection = None;
        self.next_dirty_snapshots.clear();
        self.dirty_windows.clear();
        self.removed_windows.clear();
    }

    pub(in crate::compositor) fn admission_collection(&self) -> AstreaToplevelCollection {
        if let Some(transaction) = self.transaction.as_ref() {
            transaction.target.clone()
        } else {
            AstreaToplevelCollection {
                snapshots: self.canonical.clone(),
                eligible_ids: self.canonical_eligible_ids.clone(),
                total: self.canonical_total,
            }
        }
    }
}
