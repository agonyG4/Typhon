use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum XdgConstructionState {
    AssociatedUnconstructed,
    ConstructedToplevel,
    ConstructedPopup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum XdgMapState {
    AwaitingInitialEmptyCommit,
    AwaitingInitialConfigureAck,
    UnmappedConfigured,
    Mapped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct XdgConfigureRecord {
    pub(in crate::compositor) serial: u32,
    pub(in crate::compositor) acknowledged: bool,
    pub(in crate::compositor) superseded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::compositor) struct XdgSurfaceLifecycle {
    pub(in crate::compositor) construction: XdgConstructionState,
    pub(in crate::compositor) map_state: XdgMapState,
    pub(in crate::compositor) initial_configure_sent: bool,
    pub(in crate::compositor) initial_configure_acked: bool,
    pub(in crate::compositor) currently_mapped: bool,
    initial_empty_commit_received: bool,
    pub(in crate::compositor) configures: VecDeque<XdgConfigureRecord>,
    pub(in crate::compositor) last_acked_serial: Option<u32>,
}

impl Default for XdgSurfaceLifecycle {
    fn default() -> Self {
        Self {
            construction: XdgConstructionState::AssociatedUnconstructed,
            map_state: XdgMapState::AwaitingInitialEmptyCommit,
            initial_configure_sent: false,
            initial_configure_acked: false,
            currently_mapped: false,
            initial_empty_commit_received: false,
            configures: VecDeque::new(),
            last_acked_serial: None,
        }
    }
}

impl XdgSurfaceLifecycle {
    pub(in crate::compositor) fn construct_toplevel(&mut self) -> Result<(), ()> {
        if self.construction != XdgConstructionState::AssociatedUnconstructed {
            return Err(());
        }
        self.construction = XdgConstructionState::ConstructedToplevel;
        Ok(())
    }

    pub(in crate::compositor) fn construct_popup(&mut self) -> Result<(), ()> {
        if self.construction != XdgConstructionState::AssociatedUnconstructed {
            return Err(());
        }
        self.construction = XdgConstructionState::ConstructedPopup;
        Ok(())
    }

    pub(in crate::compositor) fn is_constructed(&self) -> bool {
        !matches!(
            self.construction,
            XdgConstructionState::AssociatedUnconstructed
        )
    }

    pub(in crate::compositor) fn can_commit_buffer(&self) -> bool {
        self.initial_configure_acked
    }

    pub(in crate::compositor) fn needs_configure(&self) -> bool {
        self.initial_empty_commit_received && !self.initial_configure_sent
    }

    pub(in crate::compositor) fn has_outstanding_configure(&self) -> bool {
        self.configures
            .iter()
            .any(|configure| !configure.acknowledged)
    }

    pub(in crate::compositor) fn record_configure(&mut self, serial: u32) {
        for configure in &mut self.configures {
            configure.superseded = true;
        }
        self.configures.push_back(XdgConfigureRecord {
            serial,
            acknowledged: false,
            superseded: false,
        });
        self.initial_configure_sent = true;
        if !self.initial_configure_acked {
            self.map_state = XdgMapState::AwaitingInitialConfigureAck;
        }
    }

    pub(in crate::compositor) fn acknowledge(&mut self, serial: u32) -> Result<(), ()> {
        let Some(index) = self
            .configures
            .iter()
            .position(|configure| configure.serial == serial)
        else {
            return Err(());
        };
        if self.configures[index].acknowledged {
            return Err(());
        }
        for _ in 0..=index {
            let Some(mut configure) = self.configures.pop_front() else {
                return Err(());
            };
            configure.acknowledged = configure.serial == serial;
            if configure.serial == serial {
                self.last_acked_serial = Some(serial);
            }
        }
        self.initial_configure_acked = true;
        self.map_state = if self.currently_mapped {
            XdgMapState::Mapped
        } else {
            XdgMapState::UnmappedConfigured
        };
        Ok(())
    }

    pub(in crate::compositor) fn mark_initial_empty_commit(&mut self) -> bool {
        if self.map_state != XdgMapState::AwaitingInitialEmptyCommit {
            return false;
        }
        self.initial_empty_commit_received = true;
        self.map_state = XdgMapState::AwaitingInitialConfigureAck;
        true
    }

    pub(in crate::compositor) fn mark_buffer_commit(&mut self) {
        if self.initial_configure_acked {
            self.currently_mapped = true;
            self.map_state = XdgMapState::Mapped;
        }
    }

    pub(in crate::compositor) fn mark_unmapped(&mut self) {
        self.initial_configure_sent = false;
        self.initial_configure_acked = false;
        self.currently_mapped = false;
        // The null-buffer commit is the empty commit that starts the fresh
        // configure handshake required before a later remap.
        self.initial_empty_commit_received = true;
        self.configures.clear();
        self.last_acked_serial = None;
        self.map_state = XdgMapState::AwaitingInitialEmptyCommit;
    }

    pub(in crate::compositor) fn begin_empty_or_unmap_commit(&mut self) {
        if !self.initial_empty_commit_received
            && !self.initial_configure_sent
            && !self.initial_configure_acked
        {
            self.initial_empty_commit_received = true;
            self.map_state = XdgMapState::AwaitingInitialConfigureAck;
        } else {
            self.mark_unmapped();
        }
    }
}

impl CompositorState {
    pub(in crate::compositor) fn has_unpublished_surface_work(&self, surface_id: u32) -> bool {
        self.pending_explicit_sync_commits
            .iter()
            .any(|commit| commit.surface_id == surface_id)
            || self
                .pending_surface_tree_transactions
                .iter()
                .any(|transaction| {
                    transaction
                        .nodes
                        .iter()
                        .any(|(node_surface_id, _)| *node_surface_id == surface_id)
                })
    }

    pub(in crate::compositor) fn retire_unpublished_work_for_xdg_role(
        &mut self,
        surface_id: u32,
        reason: AcquireWatchCancelReason,
    ) {
        let pending_commits_before = self.pending_explicit_sync_commits.len();
        let pending_trees_before = self.pending_surface_tree_transactions.len();
        let acquire_changes_before = self.pending_acquire_watch_changes.len();

        let callbacks = self.cancel_pending_acquire_commits_for_surface(surface_id, reason);
        self.complete_frame_callbacks(callbacks);
        self.cancel_pending_surface_trees_for_surface(surface_id, reason);

        let pending_commits_retired =
            pending_commits_before.saturating_sub(self.pending_explicit_sync_commits.len());
        let pending_trees_retired =
            pending_trees_before.saturating_sub(self.pending_surface_tree_transactions.len());
        let acquire_watches_cancelled = self.pending_acquire_watch_changes
            [acquire_changes_before..]
            .iter()
            .filter(|change| {
                matches!(
                    change,
                    AcquireWatchChange::Cancel {
                        reason: AcquireWatchCancelReason::RoleDestroyed,
                        ..
                    }
                )
            })
            .count();

        self.compliance_metrics
            .note_xdg_role_destroyed_pending_commits_retired(pending_commits_retired);
        self.compliance_metrics
            .note_xdg_role_destroyed_pending_trees_retired(pending_trees_retired);
        self.compliance_metrics
            .note_xdg_role_destroyed_acquire_watches_cancelled(acquire_watches_cancelled);

        if surface_tree_debug_enabled()
            && (pending_commits_retired > 0 || pending_trees_retired > 0)
        {
            eprintln!(
                "oblivion-one compositor: xdg_role_destroyed_work_retired surface={surface_id} pending_commits={pending_commits_retired} pending_trees={pending_trees_retired} acquire_watches={acquire_watches_cancelled} reason={reason:?}"
            );
        }
    }

    pub(in crate::compositor) fn xdg_surface_lifecycle(
        &self,
        surface_id: u32,
    ) -> Option<&XdgSurfaceLifecycle> {
        self.xdg_surface_lifecycles.get(&surface_id)
    }

    pub(in crate::compositor) fn xdg_surface_lifecycle_mut(
        &mut self,
        surface_id: u32,
    ) -> Option<&mut XdgSurfaceLifecycle> {
        self.xdg_surface_lifecycles.get_mut(&surface_id)
    }

    pub(in crate::compositor) fn xdg_surface_is_configured(&self, surface_id: u32) -> bool {
        self.xdg_surface_lifecycle(surface_id)
            .is_some_and(XdgSurfaceLifecycle::can_commit_buffer)
    }

    pub(in crate::compositor) fn xdg_surface_is_constructed(&self, surface_id: u32) -> bool {
        self.xdg_surface_lifecycle(surface_id)
            .is_some_and(XdgSurfaceLifecycle::is_constructed)
    }

    pub(in crate::compositor) fn record_xdg_configure(&mut self, surface_id: u32, serial: u32) {
        if let Some(lifecycle) = self.xdg_surface_lifecycle_mut(surface_id) {
            lifecycle.record_configure(serial);
        }
    }

    pub(in crate::compositor) fn acknowledge_xdg_configure(
        &mut self,
        surface_id: u32,
        serial: u32,
    ) -> bool {
        self.xdg_surface_lifecycle_mut(surface_id)
            .is_some_and(|lifecycle| lifecycle.acknowledge(serial).is_ok())
    }

    pub(in crate::compositor) fn mark_xdg_empty_commit(&mut self, surface_id: u32) -> bool {
        self.xdg_surface_lifecycle_mut(surface_id)
            .is_some_and(XdgSurfaceLifecycle::mark_initial_empty_commit)
    }

    pub(in crate::compositor) fn mark_xdg_buffer_commit(&mut self, surface_id: u32) {
        if let Some(lifecycle) = self.xdg_surface_lifecycle_mut(surface_id) {
            lifecycle.mark_buffer_commit();
        }
    }

    pub(in crate::compositor) fn begin_xdg_empty_or_unmap_commit(&mut self, surface_id: u32) {
        if let Some(lifecycle) = self.xdg_surface_lifecycle_mut(surface_id) {
            lifecycle.begin_empty_or_unmap_commit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn initialized_lifecycle() -> XdgSurfaceLifecycle {
        let mut lifecycle = XdgSurfaceLifecycle::default();
        assert!(lifecycle.mark_initial_empty_commit());
        lifecycle.record_configure(1);
        assert!(lifecycle.acknowledge(1).is_ok());
        lifecycle.mark_buffer_commit();
        lifecycle
    }

    #[test]
    fn first_buffer_requires_initial_configure_ack() {
        let mut lifecycle = XdgSurfaceLifecycle::default();
        assert!(!lifecycle.can_commit_buffer());
        assert!(lifecycle.mark_initial_empty_commit());
        lifecycle.record_configure(1);
        assert!(!lifecycle.can_commit_buffer());
        assert!(lifecycle.acknowledge(1).is_ok());
        assert!(lifecycle.can_commit_buffer());
    }

    #[test]
    fn mapped_surface_accepts_existing_valid_content_with_outstanding_configure() {
        let mut lifecycle = initialized_lifecycle();
        lifecycle.record_configure(2);

        assert!(lifecycle.has_outstanding_configure());
        assert!(lifecycle.can_commit_buffer());
    }

    #[test]
    fn acknowledging_newest_configure_retires_older_records() {
        let mut lifecycle = initialized_lifecycle();
        lifecycle.record_configure(2);
        lifecycle.record_configure(3);

        assert!(lifecycle.acknowledge(3).is_ok());
        assert!(!lifecycle.has_outstanding_configure());
        assert_eq!(lifecycle.last_acked_serial, Some(3));
        assert!(lifecycle.can_commit_buffer());
    }

    #[test]
    fn unknown_configure_acknowledgement_is_rejected() {
        let mut lifecycle = initialized_lifecycle();
        lifecycle.record_configure(2);

        assert!(lifecycle.acknowledge(99).is_err());
        assert!(lifecycle.has_outstanding_configure());
        assert!(lifecycle.can_commit_buffer());
    }

    #[test]
    fn unmap_requires_a_fresh_configure_ack_before_remap() {
        let mut lifecycle = initialized_lifecycle();
        lifecycle.mark_unmapped();
        assert!(!lifecycle.can_commit_buffer());

        assert!(lifecycle.needs_configure());
        lifecycle.record_configure(4);
        assert!(!lifecycle.can_commit_buffer());
        assert!(lifecycle.acknowledge(4).is_ok());
        assert!(lifecycle.can_commit_buffer());
    }

    #[test]
    fn configure_sequences_do_not_regress_initialized_state() {
        let mut lifecycle = initialized_lifecycle();
        for serial in 2..=32 {
            lifecycle.record_configure(serial);
            assert!(lifecycle.can_commit_buffer());
            assert!(lifecycle.acknowledge(serial).is_ok());
            assert!(lifecycle.can_commit_buffer());
        }
    }

    #[test]
    fn bounded_configure_model_never_accepts_pre_initial_buffer() {
        let mut lifecycle = XdgSurfaceLifecycle::default();
        let mut random = 0x5eed_u32;

        for step in 0..256 {
            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            match random % 5 {
                0 => {
                    lifecycle.mark_initial_empty_commit();
                }
                1 => {
                    lifecycle.record_configure(step + 1);
                }
                2 => {
                    let serial = if random & 1 == 0 { step + 1 } else { random };
                    let _ = lifecycle.acknowledge(serial);
                }
                3 => {
                    lifecycle.mark_buffer_commit();
                }
                _ => {
                    lifecycle.mark_unmapped();
                }
            }

            if !lifecycle.can_commit_buffer() {
                assert!(
                    lifecycle.last_acked_serial.is_none()
                        || lifecycle.map_state == XdgMapState::AwaitingInitialConfigureAck
                );
            }
        }
    }

    #[test]
    fn reassociation_guard_detects_unpublished_surface_tree_work() {
        let mut state = CompositorState::default();
        state
            .pending_surface_tree_transactions
            .push(PendingSurfaceTreeTransaction {
                root_surface_id: 7,
                nodes: vec![(7, empty_cached_subsurface_commit())],
                dependencies: Vec::new(),
                received_at: Instant::now(),
            });

        assert!(state.has_unpublished_surface_work(7));
        assert!(!state.has_unpublished_surface_work(8));
    }

    #[test]
    fn role_retirement_preserves_unrelated_tree_work_and_is_idempotent() {
        let mut state = CompositorState::default();
        state.pending_surface_tree_transactions.extend([
            PendingSurfaceTreeTransaction {
                root_surface_id: 7,
                nodes: vec![(7, empty_cached_subsurface_commit())],
                dependencies: Vec::new(),
                received_at: Instant::now(),
            },
            PendingSurfaceTreeTransaction {
                root_surface_id: 8,
                nodes: vec![(8, empty_cached_subsurface_commit())],
                dependencies: Vec::new(),
                received_at: Instant::now(),
            },
        ]);

        state.retire_unpublished_work_for_xdg_role(7, AcquireWatchCancelReason::RoleDestroyed);
        assert!(!state.has_unpublished_surface_work(7));
        assert!(state.has_unpublished_surface_work(8));
        assert_eq!(
            state
                .compliance_metrics
                .xdg_role_destroyed_pending_trees_retired,
            1
        );

        state.retire_unpublished_work_for_xdg_role(7, AcquireWatchCancelReason::RoleDestroyed);
        assert_eq!(
            state
                .compliance_metrics
                .xdg_role_destroyed_pending_trees_retired,
            1
        );
        assert!(state.has_unpublished_surface_work(8));
    }
}
