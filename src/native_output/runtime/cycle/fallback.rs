use super::super::*;

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectFallbackReason {
    TestOnlyRejected,
    RealSubmitRejected,
    WorkerAdmissionRejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectFallbackTracker {
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) started_at_refresh_sequence: u64,
    pub(crate) cycles: u64,
    pub(crate) reason: DirectFallbackReason,
    last_observed_refresh_sequence: u64,
}

impl DirectFallbackTracker {
    pub(crate) fn start(
        slot: &mut Option<Self>,
        transaction_id: OutputTransactionId,
        refresh_sequence: u64,
        reason: DirectFallbackReason,
    ) -> bool {
        if slot.is_some() {
            return false;
        }
        *slot = Some(Self {
            transaction_id,
            started_at_refresh_sequence: refresh_sequence,
            cycles: 0,
            reason,
            last_observed_refresh_sequence: refresh_sequence,
        });
        true
    }

    pub(crate) fn observe_refresh(&mut self, refresh_sequence: u64) {
        if refresh_sequence <= self.last_observed_refresh_sequence {
            return;
        }
        self.last_observed_refresh_sequence = refresh_sequence;
        self.cycles = refresh_sequence.saturating_sub(self.started_at_refresh_sequence);
    }
}

impl NativeRuntime {
    pub(crate) fn begin_direct_fallback(
        &mut self,
        transaction_id: OutputTransactionId,
        reason: DirectFallbackReason,
    ) {
        let _ = DirectFallbackTracker::start(
            &mut self.direct_fallback_tracker,
            transaction_id,
            self.last_refresh_sequence,
            reason,
        );
    }

    pub(crate) fn abandon_direct_fallback(&mut self) {
        self.scanout.note_direct_fallback_cycles(0);
        self.direct_fallback_tracker = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU64;

    fn transaction_id() -> OutputTransactionId {
        OutputTransactionId::new(NonZeroU64::new(7).expect("transaction ID"))
    }

    #[test]
    fn test_rejection_starts_fallback_tracker() {
        let mut tracker = None;
        assert!(DirectFallbackTracker::start(
            &mut tracker,
            transaction_id(),
            10,
            DirectFallbackReason::TestOnlyRejected,
        ));
        assert_eq!(tracker.expect("tracker").cycles, 0);
    }

    #[test]
    fn real_submit_rejection_starts_fallback_tracker() {
        let mut tracker = None;
        assert!(DirectFallbackTracker::start(
            &mut tracker,
            transaction_id(),
            10,
            DirectFallbackReason::RealSubmitRejected,
        ));
    }

    #[test]
    fn refresh_cycles_increment_fallback_tracker_once_each() {
        let mut tracker = None;
        DirectFallbackTracker::start(
            &mut tracker,
            transaction_id(),
            10,
            DirectFallbackReason::TestOnlyRejected,
        );
        let tracker = tracker.as_mut().expect("tracker");
        tracker.observe_refresh(11);
        tracker.observe_refresh(11);
        tracker.observe_refresh(12);
        assert_eq!(tracker.cycles, 2);
    }

    #[test]
    fn composed_pageflip_finishes_fallback_tracker() {
        let mut tracker = None;
        DirectFallbackTracker::start(
            &mut tracker,
            transaction_id(),
            10,
            DirectFallbackReason::RealSubmitRejected,
        );
        let mut tracker = tracker.take().expect("tracker");
        tracker.observe_refresh(12);
        assert_eq!(tracker.cycles, 2);
    }

    #[test]
    fn duplicate_rejection_does_not_duplicate_tracker() {
        let mut tracker = None;
        assert!(DirectFallbackTracker::start(
            &mut tracker,
            transaction_id(),
            10,
            DirectFallbackReason::TestOnlyRejected,
        ));
        assert!(!DirectFallbackTracker::start(
            &mut tracker,
            transaction_id(),
            10,
            DirectFallbackReason::RealSubmitRejected,
        ));
    }

    #[test]
    fn ordinary_eligibility_blocker_does_not_start_tracker() {
        let tracker: Option<DirectFallbackTracker> = None;
        assert!(tracker.is_none());
    }

    #[test]
    fn shutdown_abandonment_does_not_count_as_composited_fallback() {
        let mut tracker = None;
        DirectFallbackTracker::start(
            &mut tracker,
            transaction_id(),
            10,
            DirectFallbackReason::WorkerAdmissionRejected,
        );
        tracker = None;
        assert!(tracker.is_none());
    }
}
