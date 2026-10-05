use crate::native::presentation_deadline::MonotonicTimestampNs;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationDomain {
    FixedVsync,
    VrrWindow,
    AsyncImmediate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationTiming {
    FixedVsync {
        target_time: MonotonicTimestampNs,
        refresh_interval: Duration,
    },
    VrrWindow {
        anchor: MonotonicTimestampNs,
        earliest_present: MonotonicTimestampNs,
        preferred_present: MonotonicTimestampNs,
        /// Soft service/starvation target, never an expiry or latest-valid-present time.
        service_deadline: Option<MonotonicTimestampNs>,
        shortest_interval: Duration,
        /// `None` means the minimum refresh rate is unknown.
        longest_interval: Option<Duration>,
    },
    AsyncImmediate {
        not_before: MonotonicTimestampNs,
    },
}

impl PresentationTiming {
    pub const fn domain(self) -> PresentationDomain {
        match self {
            Self::FixedVsync { .. } => PresentationDomain::FixedVsync,
            Self::VrrWindow { .. } => PresentationDomain::VrrWindow,
            Self::AsyncImmediate { .. } => PresentationDomain::AsyncImmediate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PresentationDomain, PresentationTiming};
    use crate::native::buffering::{PresentationOpportunity, PresentationOpportunityId};
    use crate::native::presentation_deadline::MonotonicTimestampNs;
    use std::time::Duration;

    #[test]
    fn presentation_timing_fixed_opportunity_preserves_identity_and_metadata() {
        let id = PresentationOpportunityId::new(7, 23);
        let target_time = MonotonicTimestampNs::new(1_234_567);
        let refresh_interval = Duration::from_nanos(8_333_333);
        let opportunity = PresentationOpportunity::fixed_vsync(id, target_time, refresh_interval);

        assert_eq!(opportunity.id(), id);
        assert_eq!(
            opportunity.timing(),
            PresentationTiming::FixedVsync {
                target_time,
                refresh_interval,
            }
        );
        assert_eq!(opportunity.domain(), PresentationDomain::FixedVsync);
    }

    #[test]
    fn presentation_timing_fixed_successor_advances_one_interval_and_sequence() {
        let interval = Duration::from_nanos(8_333_333);
        let opportunity = PresentationOpportunity::fixed_vsync(
            PresentationOpportunityId::new(7, 23),
            MonotonicTimestampNs::new(1_234_567),
            interval,
        );

        let successor = opportunity.fixed_vsync_successor().unwrap();

        assert_eq!(successor.id(), PresentationOpportunityId::new(7, 24));
        assert_eq!(successor.domain(), PresentationDomain::FixedVsync);
        assert_eq!(
            successor.timing(),
            PresentationTiming::FixedVsync {
                target_time: MonotonicTimestampNs::new(9_567_900),
                refresh_interval: interval,
            }
        );
    }

    #[test]
    fn presentation_timing_vrr_window_has_no_periodic_successor() {
        let opportunity = PresentationOpportunity::new(
            PresentationOpportunityId::new(3, 41),
            PresentationTiming::VrrWindow {
                anchor: MonotonicTimestampNs::new(1_000_000_000),
                earliest_present: MonotonicTimestampNs::new(1_002_000_000),
                preferred_present: MonotonicTimestampNs::new(1_004_000_000),
                service_deadline: Some(MonotonicTimestampNs::new(1_020_000_000)),
                shortest_interval: Duration::from_millis(5),
                longest_interval: None,
            },
        );

        assert_eq!(opportunity.domain(), PresentationDomain::VrrWindow);
        assert_eq!(opportunity.fixed_vsync_successor(), None);
    }

    #[test]
    fn presentation_timing_metadata_does_not_change_opportunity_identity() {
        let id = PresentationOpportunityId::new(3, 41);
        let fixed = PresentationOpportunity::fixed_vsync(
            id,
            MonotonicTimestampNs::new(1_000_000_000),
            Duration::from_millis(16),
        );
        let variable = PresentationOpportunity::new(
            id,
            PresentationTiming::VrrWindow {
                anchor: MonotonicTimestampNs::new(1_000_000_000),
                earliest_present: MonotonicTimestampNs::new(1_002_000_000),
                preferred_present: MonotonicTimestampNs::new(1_004_000_000),
                service_deadline: None,
                shortest_interval: Duration::from_millis(5),
                longest_interval: None,
            },
        );

        assert_eq!(fixed.id(), variable.id());
        assert_ne!(fixed.timing(), variable.timing());
    }

    #[test]
    fn presentation_timing_async_immediate_has_no_refresh_successor() {
        let opportunity = PresentationOpportunity::new(
            PresentationOpportunityId::new(8, 19),
            PresentationTiming::AsyncImmediate {
                not_before: MonotonicTimestampNs::new(4_200_000),
            },
        );

        assert_eq!(opportunity.domain(), PresentationDomain::AsyncImmediate);
        assert_eq!(opportunity.fixed_vsync_successor(), None);
    }

    #[test]
    fn presentation_timing_domain_is_derived_from_variant() {
        let fixed = PresentationTiming::FixedVsync {
            target_time: MonotonicTimestampNs::new(10),
            refresh_interval: Duration::from_millis(16),
        };
        let vrr = PresentationTiming::VrrWindow {
            anchor: MonotonicTimestampNs::new(20),
            earliest_present: MonotonicTimestampNs::new(21),
            preferred_present: MonotonicTimestampNs::new(22),
            service_deadline: None,
            shortest_interval: Duration::from_millis(5),
            longest_interval: None,
        };
        let immediate = PresentationTiming::AsyncImmediate {
            not_before: MonotonicTimestampNs::new(30),
        };

        assert_eq!(fixed.domain(), PresentationDomain::FixedVsync);
        assert_eq!(vrr.domain(), PresentationDomain::VrrWindow);
        assert_eq!(immediate.domain(), PresentationDomain::AsyncImmediate);
    }
}
