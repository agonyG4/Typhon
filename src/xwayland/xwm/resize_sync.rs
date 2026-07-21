use std::collections::{HashMap, HashSet};
use std::num::NonZeroU64;

use super::{X11Geometry, X11WindowHandle, XwaylandGeneration};
use crate::compositor::SurfaceCommitSequence;

pub const RESIZE_SYNC_TIMEOUT_NS: u64 = 10_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeSyncState {
    Idle,
    ConfigureSent {
        counter_value: u64,
        deadline_ns: u64,
    },
    AckObserved {
        counter_value: u64,
        deadline_ns: u64,
    },
    AckedWaitingCommit {
        counter_value: u64,
        association_serial: NonZeroU64,
        commit_floor: SurfaceCommitSequence,
        deadline_ns: u64,
    },
    Presented {
        counter_value: u64,
    },
    FallbackUnsynchronized,
}

impl ResizeSyncState {
    pub(crate) const fn counter_value(self) -> Option<u64> {
        match self {
            Self::ConfigureSent { counter_value, .. }
            | Self::AckObserved { counter_value, .. }
            | Self::AckedWaitingCommit { counter_value, .. }
            | Self::Presented { counter_value } => Some(counter_value),
            Self::Idle | Self::FallbackUnsynchronized => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResizeSyncCommit {
    Deferred,
    Presented,
    FallbackPresented,
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TimedOutResize {
    pub(crate) counter_value: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResizeSyncDesired {
    pub(crate) geometry: X11Geometry,
    pub(crate) final_pending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResizeSyncTransaction {
    id: u64,
    geometry: X11Geometry,
    final_pending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeSyncError {
    InvalidCounter,
    AlreadyPending,
}

impl std::fmt::Display for ResizeSyncError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCounter => formatter.write_str("resize sync counter must be nonzero"),
            Self::AlreadyPending => formatter.write_str("resize sync is already pending"),
        }
    }
}

impl std::error::Error for ResizeSyncError {}

#[derive(Debug, Default)]
pub(crate) struct ResizeSyncTracker {
    states: HashMap<X11WindowHandle, ResizeSyncState>,
    transactions: HashMap<X11WindowHandle, ResizeSyncTransaction>,
    desired: HashMap<X11WindowHandle, ResizeSyncDesired>,
    next_transaction_ids: HashMap<X11WindowHandle, u64>,
    sync_disabled: HashSet<X11WindowHandle>,
}

impl ResizeSyncTracker {
    pub(crate) fn begin_transaction(
        &mut self,
        handle: X11WindowHandle,
        counter_value: u64,
        deadline_ns: u64,
        geometry: X11Geometry,
        final_pending: bool,
    ) -> Result<(), ResizeSyncError> {
        if counter_value == 0 {
            return Err(ResizeSyncError::InvalidCounter);
        }
        if !matches!(self.state(handle), ResizeSyncState::Idle) {
            return Err(ResizeSyncError::AlreadyPending);
        }
        let next_id = self.next_transaction_ids.entry(handle).or_insert(0);
        *next_id = next_id.saturating_add(1).max(1);
        self.transactions.insert(
            handle,
            ResizeSyncTransaction {
                id: *next_id,
                geometry,
                final_pending,
            },
        );
        self.states.insert(
            handle,
            ResizeSyncState::ConfigureSent {
                counter_value,
                deadline_ns,
            },
        );
        Ok(())
    }

    pub(crate) fn queue_desired(
        &mut self,
        handle: X11WindowHandle,
        geometry: X11Geometry,
        final_pending: bool,
    ) -> bool {
        let desired = ResizeSyncDesired {
            geometry,
            final_pending,
        };
        if self.desired.get(&handle).copied() == Some(desired) {
            return false;
        }
        self.desired.insert(handle, desired);
        true
    }

    pub(crate) fn take_desired(&mut self, handle: X11WindowHandle) -> Option<ResizeSyncDesired> {
        self.desired.remove(&handle)
    }

    pub(crate) fn desired(&self, handle: X11WindowHandle) -> Option<ResizeSyncDesired> {
        self.desired.get(&handle).copied()
    }

    pub(crate) fn is_pending(&self, handle: X11WindowHandle) -> bool {
        !matches!(self.state(handle), ResizeSyncState::Idle)
    }

    pub(crate) fn transaction_id(&self, handle: X11WindowHandle) -> Option<u64> {
        self.transactions
            .get(&handle)
            .map(|transaction| transaction.id)
    }

    pub(crate) fn transaction(&self, handle: X11WindowHandle) -> Option<(u64, X11Geometry, bool)> {
        self.transactions.get(&handle).map(|transaction| {
            (
                transaction.id,
                transaction.geometry,
                transaction.final_pending,
            )
        })
    }

    pub(crate) fn acknowledge(&mut self, handle: X11WindowHandle, counter_value: u64) -> bool {
        let Some(ResizeSyncState::ConfigureSent {
            counter_value: expected,
            deadline_ns,
        }) = self.states.get(&handle).copied()
        else {
            return false;
        };
        if expected != counter_value {
            return false;
        }
        self.states.insert(
            handle,
            ResizeSyncState::AckObserved {
                counter_value,
                deadline_ns,
            },
        );
        true
    }

    pub(crate) fn release_commits(
        &mut self,
        handle: X11WindowHandle,
        counter_value: u64,
        association_serial: NonZeroU64,
        commit_floor: SurfaceCommitSequence,
    ) -> bool {
        let Some(ResizeSyncState::AckObserved {
            counter_value: expected,
            deadline_ns,
        }) = self.states.get(&handle).copied()
        else {
            return false;
        };
        if expected != counter_value {
            return false;
        }
        self.states.insert(
            handle,
            ResizeSyncState::AckedWaitingCommit {
                counter_value,
                association_serial,
                commit_floor,
                deadline_ns,
            },
        );
        true
    }

    pub(crate) fn note_commit(
        &mut self,
        handle: X11WindowHandle,
        association_serial: NonZeroU64,
        commit_sequence: SurfaceCommitSequence,
    ) -> ResizeSyncCommit {
        match self.state(handle) {
            ResizeSyncState::AckedWaitingCommit {
                counter_value,
                association_serial: expected_serial,
                commit_floor,
                ..
            } if expected_serial == association_serial && commit_sequence > commit_floor => {
                self.states
                    .insert(handle, ResizeSyncState::Presented { counter_value });
                ResizeSyncCommit::Presented
            }
            ResizeSyncState::FallbackUnsynchronized => {
                self.states
                    .insert(handle, ResizeSyncState::Presented { counter_value: 0 });
                ResizeSyncCommit::FallbackPresented
            }
            ResizeSyncState::ConfigureSent { .. } | ResizeSyncState::AckObserved { .. } => {
                ResizeSyncCommit::Deferred
            }
            ResizeSyncState::AckedWaitingCommit { .. } => ResizeSyncCommit::Ignored,
            ResizeSyncState::Idle | ResizeSyncState::Presented { .. } => ResizeSyncCommit::Ignored,
        }
    }

    pub(crate) fn complete(&mut self, handle: X11WindowHandle) -> bool {
        if !matches!(self.state(handle), ResizeSyncState::Presented { .. }) {
            return false;
        }
        self.states.insert(handle, ResizeSyncState::Idle);
        self.transactions.remove(&handle);
        true
    }

    pub(crate) fn timeout(
        &mut self,
        handle: X11WindowHandle,
        now_ns: u64,
    ) -> Option<TimedOutResize> {
        let timed_out = match self.state(handle) {
            ResizeSyncState::ConfigureSent {
                counter_value,
                deadline_ns,
            }
            | ResizeSyncState::AckObserved {
                counter_value,
                deadline_ns,
            }
            | ResizeSyncState::AckedWaitingCommit {
                counter_value,
                deadline_ns,
                ..
            } if now_ns >= deadline_ns => Some(TimedOutResize { counter_value }),
            _ => None,
        };
        if timed_out.is_some() {
            self.states
                .insert(handle, ResizeSyncState::FallbackUnsynchronized);
            self.sync_disabled.insert(handle);
        }
        timed_out
    }

    pub(crate) fn next_deadline_ns(&self) -> Option<u64> {
        self.states
            .values()
            .filter_map(|state| match state {
                ResizeSyncState::ConfigureSent { deadline_ns, .. }
                | ResizeSyncState::AckObserved { deadline_ns, .. }
                | ResizeSyncState::AckedWaitingCommit { deadline_ns, .. } => Some(*deadline_ns),
                ResizeSyncState::Idle
                | ResizeSyncState::Presented { .. }
                | ResizeSyncState::FallbackUnsynchronized => None,
            })
            .min()
    }

    pub(crate) fn expired_handles(&self, now_ns: u64) -> Vec<X11WindowHandle> {
        self.states
            .iter()
            .filter_map(|(handle, state)| match state {
                ResizeSyncState::ConfigureSent { deadline_ns, .. }
                | ResizeSyncState::AckObserved { deadline_ns, .. }
                | ResizeSyncState::AckedWaitingCommit { deadline_ns, .. }
                    if now_ns >= *deadline_ns =>
                {
                    Some(*handle)
                }
                _ => None,
            })
            .collect()
    }

    pub(crate) fn state(&self, handle: X11WindowHandle) -> ResizeSyncState {
        self.states
            .get(&handle)
            .copied()
            .unwrap_or(ResizeSyncState::Idle)
    }

    pub(crate) fn clear(&mut self, handle: X11WindowHandle) {
        self.states.remove(&handle);
        self.transactions.remove(&handle);
        self.desired.remove(&handle);
        self.sync_disabled.remove(&handle);
    }

    pub(crate) fn disable_after_timeout(&mut self, handle: X11WindowHandle) {
        self.sync_disabled.insert(handle);
    }

    pub(crate) fn sync_disabled(&self, handle: X11WindowHandle) -> bool {
        self.sync_disabled.contains(&handle)
    }

    pub(crate) fn reenable_sync(&mut self, handle: X11WindowHandle) {
        self.sync_disabled.remove(&handle);
    }

    pub(crate) fn clear_generation(&mut self, generation: XwaylandGeneration) {
        self.states
            .retain(|handle, _| handle.generation() != generation);
        self.transactions
            .retain(|handle, _| handle.generation() != generation);
        self.desired
            .retain(|handle, _| handle.generation() != generation);
        self.next_transaction_ids
            .retain(|handle, _| handle.generation() != generation);
        self.sync_disabled
            .retain(|handle| handle.generation() != generation);
    }

    pub(crate) fn finish_timeout(&mut self, handle: X11WindowHandle) -> bool {
        if !matches!(self.state(handle), ResizeSyncState::FallbackUnsynchronized) {
            return false;
        }
        self.states.insert(handle, ResizeSyncState::Idle);
        self.transactions.remove(&handle);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::SurfaceCommitSequence;
    use std::num::NonZeroU64;

    fn handle(generation: u64, xid: u32) -> X11WindowHandle {
        X11WindowHandle::new(
            XwaylandGeneration::new(NonZeroU64::new(generation).unwrap()),
            xid,
        )
    }

    fn association_serial(value: u64) -> NonZeroU64 {
        NonZeroU64::new(value).expect("association serial")
    }

    fn note_commit(
        tracker: &mut ResizeSyncTracker,
        window: X11WindowHandle,
        sequence: u64,
    ) -> ResizeSyncCommit {
        tracker.note_commit(
            window,
            association_serial(9),
            SurfaceCommitSequence(sequence),
        )
    }

    fn release_commits(
        tracker: &mut ResizeSyncTracker,
        window: X11WindowHandle,
        counter_value: u64,
        commit_floor: u64,
    ) {
        assert!(tracker.acknowledge(window, counter_value));
        assert!(tracker.release_commits(
            window,
            counter_value,
            association_serial(9),
            SurfaceCommitSequence(commit_floor),
        ));
    }

    #[test]
    fn sync_capable_resize_waits_for_counter_without_blocking_reactor() {
        let window = handle(1, 10);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(window, 7, 100, X11Geometry::default(), false)
            .expect("begin resize sync");
        assert_eq!(
            tracker.state(window),
            ResizeSyncState::ConfigureSent {
                counter_value: 7,
                deadline_ns: 100,
            }
        );
        assert_eq!(tracker.next_deadline_ns(), Some(100));
        assert!(!tracker.acknowledge(window, 6));
        assert_eq!(
            tracker.state(window),
            ResizeSyncState::ConfigureSent {
                counter_value: 7,
                deadline_ns: 100,
            }
        );
    }

    #[test]
    fn allow_commits_is_disabled_only_for_target_window() {
        let target = handle(1, 11);
        let other = handle(1, 12);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(target, 9, 100, X11Geometry::default(), false)
            .expect("begin target sync");
        assert!(!matches!(tracker.state(target), ResizeSyncState::Idle));
        assert_eq!(tracker.state(other), ResizeSyncState::Idle);
    }

    #[test]
    fn sync_ack_releases_matching_resize_commit() {
        let window = handle(1, 13);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(window, 12, 100, X11Geometry::default(), false)
            .expect("begin resize sync");
        release_commits(&mut tracker, window, 12, 0);
        assert_eq!(
            note_commit(&mut tracker, window, 1),
            ResizeSyncCommit::Presented
        );
        assert!(tracker.complete(window));
        assert_eq!(tracker.state(window), ResizeSyncState::Idle);
    }

    #[test]
    fn stale_counter_ack_cannot_release_newer_resize() {
        let window = handle(1, 14);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(window, 20, 100, X11Geometry::default(), false)
            .expect("first resize sync");
        release_commits(&mut tracker, window, 20, 0);
        assert_eq!(
            note_commit(&mut tracker, window, 1),
            ResizeSyncCommit::Presented
        );
        assert!(tracker.complete(window));
        tracker
            .begin_transaction(window, 21, 200, X11Geometry::default(), false)
            .expect("second resize sync");
        assert!(!tracker.acknowledge(window, 20));
        assert!(matches!(
            tracker.state(window),
            ResizeSyncState::ConfigureSent {
                counter_value: 21,
                ..
            }
        ));
    }

    #[test]
    fn timeout_falls_back_without_freezing_window() {
        let window = handle(1, 15);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(window, 30, 100, X11Geometry::default(), false)
            .expect("resize sync");
        assert!(tracker.timeout(window, 100).is_some());
        assert_eq!(
            tracker.state(window),
            ResizeSyncState::FallbackUnsynchronized
        );
        assert_eq!(
            note_commit(&mut tracker, window, 1),
            ResizeSyncCommit::FallbackPresented
        );
        assert!(tracker.complete(window));
        assert_eq!(tracker.state(window), ResizeSyncState::Idle);
    }

    #[test]
    fn destroy_during_sync_clears_counter_and_commit_gate() {
        let window = handle(1, 16);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(window, 40, 100, X11Geometry::default(), false)
            .expect("resize sync");
        tracker.clear(window);
        assert_eq!(tracker.state(window), ResizeSyncState::Idle);
        assert_eq!(
            note_commit(&mut tracker, window, 1),
            ResizeSyncCommit::Ignored
        );
    }

    #[test]
    fn generation_restart_clears_all_resize_sync_state() {
        let old_generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
        let old_window = X11WindowHandle::new(old_generation, 17);
        let new_generation = XwaylandGeneration::new(NonZeroU64::new(2).unwrap());
        let new_window = X11WindowHandle::new(new_generation, 17);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(old_window, 50, 100, X11Geometry::default(), false)
            .expect("old resize sync");
        tracker.clear_generation(old_generation);
        assert_eq!(tracker.state(old_window), ResizeSyncState::Idle);
        assert_eq!(tracker.state(new_window), ResizeSyncState::Idle);
    }

    #[test]
    fn non_sync_client_uses_immediate_configure_path() {
        let window = handle(1, 18);
        let mut tracker = ResizeSyncTracker::default();
        assert_eq!(tracker.state(window), ResizeSyncState::Idle);
        assert_eq!(
            note_commit(&mut tracker, window, 1),
            ResizeSyncCommit::Ignored
        );
    }

    #[test]
    fn second_pointer_update_does_not_replace_pending_sync_counter() {
        let window = handle(1, 19);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(
                window,
                7,
                100,
                X11Geometry {
                    width: 800,
                    height: 600,
                    ..X11Geometry::default()
                },
                false,
            )
            .expect("first transaction");
        assert!(tracker.queue_desired(
            window,
            X11Geometry {
                width: 900,
                height: 700,
                ..X11Geometry::default()
            },
            false,
        ));
        assert_eq!(tracker.transaction(window).unwrap().0, 1);
        assert_eq!(
            tracker.state(window),
            ResizeSyncState::ConfigureSent {
                counter_value: 7,
                deadline_ns: 100
            }
        );
    }

    #[test]
    fn pointer_updates_coalesce_to_latest_x11_geometry() {
        let window = handle(1, 20);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(window, 7, 100, X11Geometry::default(), false)
            .expect("transaction");
        tracker.queue_desired(
            window,
            X11Geometry {
                width: 801,
                height: 601,
                ..X11Geometry::default()
            },
            false,
        );
        tracker.queue_desired(
            window,
            X11Geometry {
                width: 802,
                height: 602,
                ..X11Geometry::default()
            },
            false,
        );
        assert_eq!(tracker.desired(window).unwrap().geometry.width, 802);
        assert_eq!(tracker.desired(window).unwrap().geometry.height, 602);
    }

    #[test]
    fn commit_before_ack_does_not_complete_resize() {
        let window = handle(1, 21);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(window, 7, 100, X11Geometry::default(), false)
            .expect("transaction");
        assert_eq!(
            note_commit(&mut tracker, window, 1),
            ResizeSyncCommit::Deferred
        );
        assert_eq!(
            tracker.state(window),
            ResizeSyncState::ConfigureSent {
                counter_value: 7,
                deadline_ns: 100
            }
        );
    }

    #[test]
    fn matching_ack_then_commit_completes_transaction() {
        let window = handle(1, 22);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(window, 7, 100, X11Geometry::default(), false)
            .expect("transaction");
        release_commits(&mut tracker, window, 7, 0);
        assert_eq!(
            note_commit(&mut tracker, window, 1),
            ResizeSyncCommit::Presented
        );
        assert!(tracker.complete(window));
        assert_eq!(tracker.state(window), ResizeSyncState::Idle);
    }

    #[test]
    fn retained_surface_readiness_never_presents_resize() {
        let window = handle(1, 122);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(window, 7, 100, X11Geometry::default(), false)
            .expect("transaction");
        assert!(tracker.acknowledge(window, 7));
        assert_eq!(
            tracker.note_commit(
                window,
                NonZeroU64::new(9).expect("association serial"),
                SurfaceCommitSequence(10),
            ),
            ResizeSyncCommit::Deferred
        );
    }

    #[test]
    fn pre_ack_commit_delivered_after_ack_is_below_commit_floor() {
        let window = handle(1, 123);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(window, 7, 100, X11Geometry::default(), false)
            .expect("transaction");
        assert_eq!(
            tracker.note_commit(
                window,
                NonZeroU64::new(9).expect("association serial"),
                SurfaceCommitSequence(10),
            ),
            ResizeSyncCommit::Deferred
        );
        assert!(tracker.acknowledge(window, 7));
        assert!(tracker.release_commits(
            window,
            7,
            NonZeroU64::new(9).expect("association serial"),
            SurfaceCommitSequence(10),
        ));
        assert_eq!(
            tracker.note_commit(
                window,
                NonZeroU64::new(9).expect("association serial"),
                SurfaceCommitSequence(10),
            ),
            ResizeSyncCommit::Ignored
        );
    }

    #[test]
    fn first_post_release_commit_presents_resize_exactly_once() {
        let window = handle(1, 124);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(window, 7, 100, X11Geometry::default(), false)
            .expect("transaction");
        assert!(tracker.acknowledge(window, 7));
        assert!(tracker.release_commits(
            window,
            7,
            NonZeroU64::new(9).expect("association serial"),
            SurfaceCommitSequence(10),
        ));
        assert_eq!(
            tracker.note_commit(
                window,
                NonZeroU64::new(9).expect("association serial"),
                SurfaceCommitSequence(11),
            ),
            ResizeSyncCommit::Presented
        );
        assert_eq!(
            tracker.note_commit(
                window,
                NonZeroU64::new(9).expect("association serial"),
                SurfaceCommitSequence(12),
            ),
            ResizeSyncCommit::Ignored
        );
    }

    #[test]
    fn commit_from_previous_association_cannot_present_resize() {
        let window = handle(1, 125);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(window, 7, 100, X11Geometry::default(), false)
            .expect("transaction");
        assert!(tracker.acknowledge(window, 7));
        assert!(tracker.release_commits(
            window,
            7,
            NonZeroU64::new(9).expect("association serial"),
            SurfaceCommitSequence(10),
        ));
        assert_eq!(
            tracker.note_commit(
                window,
                NonZeroU64::new(8).expect("old association serial"),
                SurfaceCommitSequence(11),
            ),
            ResizeSyncCommit::Ignored
        );
    }

    #[test]
    fn release_during_pending_sync_becomes_final_pending() {
        let window = handle(1, 23);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(window, 7, 100, X11Geometry::default(), false)
            .expect("transaction");
        tracker.queue_desired(
            window,
            X11Geometry {
                width: 1000,
                height: 800,
                ..X11Geometry::default()
            },
            true,
        );
        assert!(tracker.desired(window).unwrap().final_pending);
    }

    #[test]
    fn latest_coalesced_geometry_starts_after_previous_completion() {
        let window = handle(1, 25);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(window, 7, 100, X11Geometry::default(), false)
            .expect("transaction");
        let desired = X11Geometry {
            width: 1200,
            height: 900,
            ..X11Geometry::default()
        };
        tracker.queue_desired(window, desired, true);
        release_commits(&mut tracker, window, 7, 0);
        assert_eq!(
            note_commit(&mut tracker, window, 1),
            ResizeSyncCommit::Presented
        );
        assert!(tracker.complete(window));
        let desired = tracker.take_desired(window).expect("queued target");
        tracker
            .begin_transaction(window, 8, 200, desired.geometry, desired.final_pending)
            .expect("next transaction");
        assert_eq!(tracker.transaction_id(window), Some(2));
        assert_eq!(tracker.transaction(window).unwrap().1, desired.geometry);
    }

    #[test]
    fn presented_resize_chain_keeps_preview_until_final_transaction() {
        let window = handle(1, 26);
        let mut tracker = ResizeSyncTracker::default();
        let first = X11Geometry {
            width: 900,
            height: 700,
            ..X11Geometry::default()
        };
        let second = X11Geometry {
            width: 1000,
            height: 800,
            ..X11Geometry::default()
        };
        tracker
            .begin_transaction(window, 7, 100, first, false)
            .expect("first transaction");
        tracker.queue_desired(window, second, true);
        release_commits(&mut tracker, window, 7, 0);
        assert_eq!(
            note_commit(&mut tracker, window, 1),
            ResizeSyncCommit::Presented
        );
        // Completing the first transaction only advances the X11 protocol
        // chain; its queued final target still owns the resize preview.
        assert!(tracker.complete(window));
        let desired = tracker.take_desired(window).expect("final target");
        tracker
            .begin_transaction(window, 8, 200, desired.geometry, desired.final_pending)
            .expect("final transaction");
        assert!(tracker.transaction(window).unwrap().2);
        release_commits(&mut tracker, window, 8, 0);
        assert_eq!(
            note_commit(&mut tracker, window, 1),
            ResizeSyncCommit::Presented
        );
        assert!(tracker.complete(window));
        assert_eq!(tracker.state(window), ResizeSyncState::Idle);
    }

    #[test]
    fn release_after_intermediate_completion_starts_final_transaction() {
        let window = handle(1, 27);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(window, 7, 100, X11Geometry::default(), false)
            .expect("intermediate transaction");
        release_commits(&mut tracker, window, 7, 0);
        assert_eq!(
            note_commit(&mut tracker, window, 1),
            ResizeSyncCommit::Presented
        );
        assert!(tracker.complete(window));
        assert_eq!(tracker.state(window), ResizeSyncState::Idle);

        let final_geometry = X11Geometry {
            width: 1200,
            height: 800,
            ..X11Geometry::default()
        };
        tracker
            .begin_transaction(window, 8, 200, final_geometry, true)
            .expect("final transaction after idle gap");
        assert!(tracker.transaction(window).unwrap().2);
        release_commits(&mut tracker, window, 8, 0);
        assert_eq!(
            note_commit(&mut tracker, window, 1),
            ResizeSyncCommit::Presented
        );
        assert!(tracker.complete(window));
        assert_eq!(tracker.state(window), ResizeSyncState::Idle);
    }

    #[test]
    fn timeout_restores_allow_commits() {
        let window = handle(1, 24);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(window, 7, 100, X11Geometry::default(), false)
            .expect("transaction");
        assert!(tracker.timeout(window, 100).is_some());
        assert!(tracker.finish_timeout(window));
        assert_eq!(tracker.state(window), ResizeSyncState::Idle);
    }

    #[test]
    fn slow_client_watchdog_allows_ten_seconds() {
        assert_eq!(RESIZE_SYNC_TIMEOUT_NS, 10_000_000_000);
    }

    #[test]
    fn timeout_disables_sync_until_a_matching_late_ack() {
        let window = handle(1, 28);
        let mut tracker = ResizeSyncTracker::default();
        tracker
            .begin_transaction(window, 7, 100, X11Geometry::default(), false)
            .expect("transaction");
        assert!(tracker.timeout(window, 100).is_some());
        tracker.disable_after_timeout(window);
        assert!(tracker.sync_disabled(window));
        assert!(!tracker.acknowledge(window, 7));
        tracker.reenable_sync(window);
        assert!(!tracker.sync_disabled(window));
    }
}
