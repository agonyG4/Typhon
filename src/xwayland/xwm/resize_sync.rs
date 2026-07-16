use std::collections::HashMap;

use super::{X11WindowHandle, XwaylandGeneration};

pub const RESIZE_SYNC_TIMEOUT_NS: u64 = 500_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeSyncState {
    Idle,
    ConfigureSent {
        counter_value: u64,
        deadline_ns: u64,
    },
    AckedWaitingCommit {
        counter_value: u64,
        deadline_ns: u64,
    },
    Presented {
        counter_value: u64,
    },
    FallbackUnsynchronized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResizeSyncCommit {
    Deferred,
    Presented,
    FallbackPresented,
    Ignored,
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
}

impl ResizeSyncTracker {
    pub(crate) fn begin(
        &mut self,
        handle: X11WindowHandle,
        counter_value: u64,
        deadline_ns: u64,
    ) -> Result<(), ResizeSyncError> {
        if counter_value == 0 {
            return Err(ResizeSyncError::InvalidCounter);
        }
        if !matches!(self.state(handle), ResizeSyncState::Idle) {
            return Err(ResizeSyncError::AlreadyPending);
        }
        self.states.insert(
            handle,
            ResizeSyncState::ConfigureSent {
                counter_value,
                deadline_ns,
            },
        );
        Ok(())
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
            ResizeSyncState::AckedWaitingCommit {
                counter_value,
                deadline_ns,
            },
        );
        true
    }

    pub(crate) fn note_commit(&mut self, handle: X11WindowHandle) -> ResizeSyncCommit {
        match self.state(handle) {
            ResizeSyncState::AckedWaitingCommit { counter_value, .. } => {
                self.states
                    .insert(handle, ResizeSyncState::Presented { counter_value });
                ResizeSyncCommit::Presented
            }
            ResizeSyncState::FallbackUnsynchronized => {
                self.states
                    .insert(handle, ResizeSyncState::Presented { counter_value: 0 });
                ResizeSyncCommit::FallbackPresented
            }
            ResizeSyncState::ConfigureSent { .. } => ResizeSyncCommit::Deferred,
            ResizeSyncState::Idle | ResizeSyncState::Presented { .. } => ResizeSyncCommit::Ignored,
        }
    }

    pub(crate) fn complete(&mut self, handle: X11WindowHandle) -> bool {
        if !matches!(self.state(handle), ResizeSyncState::Presented { .. }) {
            return false;
        }
        self.states.insert(handle, ResizeSyncState::Idle);
        true
    }

    pub(crate) fn timeout(&mut self, handle: X11WindowHandle, now_ns: u64) -> bool {
        let timed_out = matches!(
            self.state(handle),
            ResizeSyncState::ConfigureSent { deadline_ns, .. }
                | ResizeSyncState::AckedWaitingCommit { deadline_ns, .. }
                if now_ns >= deadline_ns
        );
        if timed_out {
            self.states
                .insert(handle, ResizeSyncState::FallbackUnsynchronized);
        }
        timed_out
    }

    pub(crate) fn next_deadline_ns(&self) -> Option<u64> {
        self.states
            .values()
            .filter_map(|state| match state {
                ResizeSyncState::ConfigureSent { deadline_ns, .. }
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
    }

    pub(crate) fn clear_generation(&mut self, generation: XwaylandGeneration) {
        self.states
            .retain(|handle, _| handle.generation() != generation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU64;

    fn handle(generation: u64, xid: u32) -> X11WindowHandle {
        X11WindowHandle::new(
            XwaylandGeneration::new(NonZeroU64::new(generation).unwrap()),
            xid,
        )
    }

    #[test]
    fn sync_capable_resize_waits_for_counter_without_blocking_reactor() {
        let window = handle(1, 10);
        let mut tracker = ResizeSyncTracker::default();
        tracker.begin(window, 7, 100).expect("begin resize sync");
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
        tracker.begin(target, 9, 100).expect("begin target sync");
        assert!(!matches!(tracker.state(target), ResizeSyncState::Idle));
        assert_eq!(tracker.state(other), ResizeSyncState::Idle);
    }

    #[test]
    fn sync_ack_releases_matching_resize_commit() {
        let window = handle(1, 13);
        let mut tracker = ResizeSyncTracker::default();
        tracker.begin(window, 12, 100).expect("begin resize sync");
        assert!(tracker.acknowledge(window, 12));
        assert_eq!(tracker.note_commit(window), ResizeSyncCommit::Presented);
        assert!(tracker.complete(window));
        assert_eq!(tracker.state(window), ResizeSyncState::Idle);
    }

    #[test]
    fn stale_counter_ack_cannot_release_newer_resize() {
        let window = handle(1, 14);
        let mut tracker = ResizeSyncTracker::default();
        tracker.begin(window, 20, 100).expect("first resize sync");
        assert!(tracker.acknowledge(window, 20));
        assert_eq!(tracker.note_commit(window), ResizeSyncCommit::Presented);
        assert!(tracker.complete(window));
        tracker.begin(window, 21, 200).expect("second resize sync");
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
        tracker.begin(window, 30, 100).expect("resize sync");
        assert!(tracker.timeout(window, 100));
        assert_eq!(
            tracker.state(window),
            ResizeSyncState::FallbackUnsynchronized
        );
        assert_eq!(
            tracker.note_commit(window),
            ResizeSyncCommit::FallbackPresented
        );
        assert!(tracker.complete(window));
        assert_eq!(tracker.state(window), ResizeSyncState::Idle);
    }

    #[test]
    fn destroy_during_sync_clears_counter_and_commit_gate() {
        let window = handle(1, 16);
        let mut tracker = ResizeSyncTracker::default();
        tracker.begin(window, 40, 100).expect("resize sync");
        tracker.clear(window);
        assert_eq!(tracker.state(window), ResizeSyncState::Idle);
        assert_eq!(tracker.note_commit(window), ResizeSyncCommit::Ignored);
    }

    #[test]
    fn generation_restart_clears_all_resize_sync_state() {
        let old_generation = XwaylandGeneration::new(NonZeroU64::new(1).unwrap());
        let old_window = X11WindowHandle::new(old_generation, 17);
        let new_generation = XwaylandGeneration::new(NonZeroU64::new(2).unwrap());
        let new_window = X11WindowHandle::new(new_generation, 17);
        let mut tracker = ResizeSyncTracker::default();
        tracker.begin(old_window, 50, 100).expect("old resize sync");
        tracker.clear_generation(old_generation);
        assert_eq!(tracker.state(old_window), ResizeSyncState::Idle);
        assert_eq!(tracker.state(new_window), ResizeSyncState::Idle);
    }

    #[test]
    fn non_sync_client_uses_immediate_configure_path() {
        let window = handle(1, 18);
        let mut tracker = ResizeSyncTracker::default();
        assert_eq!(tracker.state(window), ResizeSyncState::Idle);
        assert_eq!(tracker.note_commit(window), ResizeSyncCommit::Ignored);
    }
}
