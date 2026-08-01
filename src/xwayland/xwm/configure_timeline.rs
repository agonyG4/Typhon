use std::collections::VecDeque;

use super::{X11ConfigureFlags, X11Geometry};

pub(crate) const CONFIGURE_HISTORY_LIMIT: usize = 32;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConfigureTimelineMetrics {
    pub(crate) requests_received: u64,
    pub(crate) configures_issued: u64,
    pub(crate) notifies_expected_current: u64,
    pub(crate) notifies_expected_older: u64,
    pub(crate) notifies_coalesced: u64,
    pub(crate) notifies_stale_ignored: u64,
    pub(crate) notifies_external_applied: u64,
    pub(crate) notifies_unknown_preserved: u64,
    pub(crate) rollbacks_prevented: u64,
    pub(crate) sequence_geometry_conflicts: u64,
    pub(crate) sequence_wrap_progress: u64,
    pub(crate) sequence_only_matches_rejected: u64,
    pub(crate) client_authoritative_retired_geometry_reuse: u64,
    pub(crate) ambiguous_identical_geometry_matches: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigureSource {
    ClientRequest,
    Compositor,
    ResizeSync,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExpectedConfigure {
    pub(crate) epoch: u64,
    pub(crate) geometry: X11Geometry,
    pub(crate) fields: X11ConfigureFlags,
    pub(crate) source: ConfigureSource,
    /// Full sequence returned by the outgoing x11rb `ConfigureWindow` cookie.
    pub(crate) configure_cookie_sequence: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RetiredConfigure {
    pub(crate) epoch: u64,
    pub(crate) geometry: X11Geometry,
    pub(crate) source: ConfigureSource,
    /// Retained for diagnostics; this is not an event-causal identity.
    pub(crate) configure_cookie_sequence: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfigureNotifyClassification {
    ExpectedCurrent,
    ExpectedOlder,
    ExpectedCoalesced,
    StaleRetired,
    ClientAuthoritativeRetiredReuse,
    ExternalAuthoritative,
    SequenceOnlyRejected,
    AmbiguousGeometry,
    UnknownPreserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConfigureNotifyResult {
    pub(crate) classification: ConfigureNotifyClassification,
    pub(crate) epoch: Option<u64>,
    pub(crate) geometry: X11Geometry,
    pub(crate) sequence_geometry_conflict: bool,
    pub(crate) sequence_wrap_progress: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingMatch {
    Matched {
        index: usize,
        sequence_geometry_conflict: bool,
        sequence_wrap_progress: bool,
    },
    SequenceOnlyRejected,
    Ambiguous {
        sequence_geometry_conflict: bool,
    },
    None,
}

/// Compare a full x11rb request sequence with the low 16 bits in an event.
///
/// X11 events carry only the last processed request's low 16 bits, so this is
/// equality evidence rather than a causal identity relation.
fn sequence16_eq(full: u64, event: u16) -> bool {
    full as u16 == event
}

/// Return whether `candidate` is forward of `reference` in X11's 16-bit
/// sequence space. The half-range rule makes both wrap and stale progress
/// explicit; values exactly half a ring apart are intentionally ambiguous.
fn sequence16_is_after(candidate: u16, reference: u16) -> bool {
    let distance = candidate.wrapping_sub(reference);
    distance != 0 && distance < 0x8000
}

fn sequence_progress_reaches(progress: u16, cookie: u64) -> bool {
    sequence16_eq(cookie, progress) || sequence16_is_after(progress, cookie as u16)
}

fn sequence_progress_wrapped(progress: u16, cookie: u64) -> bool {
    let reference = cookie as u16;
    let distance = u32::from(progress.wrapping_sub(reference));
    sequence16_is_after(progress, reference)
        && u32::from(reference).saturating_add(distance) > u32::from(u16::MAX)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowConfigureTimeline {
    next_epoch: u64,
    desired: X11Geometry,
    acknowledged: Option<X11Geometry>,
    pending: VecDeque<ExpectedConfigure>,
    retired: VecDeque<RetiredConfigure>,
}

impl WindowConfigureTimeline {
    pub(crate) fn new(initial: X11Geometry) -> Self {
        Self {
            next_epoch: 0,
            desired: initial,
            acknowledged: None,
            pending: VecDeque::new(),
            retired: VecDeque::new(),
        }
    }

    pub(crate) fn record(
        &mut self,
        geometry: X11Geometry,
        fields: X11ConfigureFlags,
        source: ConfigureSource,
        configure_cookie_sequence: Option<u64>,
    ) -> ExpectedConfigure {
        self.next_epoch = self.next_epoch.saturating_add(1).max(1);
        let expected = ExpectedConfigure {
            epoch: self.next_epoch,
            geometry,
            fields,
            source,
            configure_cookie_sequence,
        };
        self.desired = geometry;
        self.pending.push_back(expected);
        self.trim_pending();
        expected
    }

    pub(crate) fn notify(
        &mut self,
        geometry: X11Geometry,
        notify_progress_sequence: Option<u16>,
        external_authoritative: bool,
    ) -> ConfigureNotifyResult {
        match self.pending_match(geometry, notify_progress_sequence) {
            PendingMatch::Matched {
                index,
                sequence_geometry_conflict,
                sequence_wrap_progress,
            } => {
                let was_current = index + 1 == self.pending.len();
                let expected = self
                    .pending
                    .remove(index)
                    .expect("pending configure index was found");
                self.acknowledged = Some(expected.geometry);

                let mut coalesced = false;
                while let Some(older) = self.pending.front().copied()
                    && older.epoch < expected.epoch
                {
                    let retired = self.pending.pop_front().expect("pending front was present");
                    self.retire(retired);
                    coalesced = true;
                }

                ConfigureNotifyResult {
                    classification: if was_current {
                        if coalesced {
                            ConfigureNotifyClassification::ExpectedCoalesced
                        } else {
                            ConfigureNotifyClassification::ExpectedCurrent
                        }
                    } else {
                        ConfigureNotifyClassification::ExpectedOlder
                    },
                    epoch: Some(expected.epoch),
                    geometry,
                    sequence_geometry_conflict,
                    sequence_wrap_progress,
                }
            }
            PendingMatch::SequenceOnlyRejected => ConfigureNotifyResult {
                classification: ConfigureNotifyClassification::SequenceOnlyRejected,
                epoch: None,
                geometry,
                sequence_geometry_conflict: false,
                sequence_wrap_progress: false,
            },
            PendingMatch::Ambiguous {
                sequence_geometry_conflict,
            } => ConfigureNotifyResult {
                classification: ConfigureNotifyClassification::AmbiguousGeometry,
                epoch: None,
                geometry,
                sequence_geometry_conflict,
                sequence_wrap_progress: false,
            },
            PendingMatch::None => {
                let retired = self
                    .retired
                    .iter()
                    .find(|retired| retired.geometry == geometry);
                if let Some(retired) = retired {
                    let delayed_self_configure = external_authoritative
                        && notify_progress_sequence.is_some_and(|sequence| {
                            retired
                                .configure_cookie_sequence
                                .is_some_and(|cookie| sequence16_eq(cookie, sequence))
                        });
                    return ConfigureNotifyResult {
                        classification: if external_authoritative && !delayed_self_configure {
                            ConfigureNotifyClassification::ClientAuthoritativeRetiredReuse
                        } else {
                            ConfigureNotifyClassification::StaleRetired
                        },
                        epoch: Some(retired.epoch),
                        geometry,
                        sequence_geometry_conflict: false,
                        sequence_wrap_progress: false,
                    };
                }

                ConfigureNotifyResult {
                    classification: if external_authoritative {
                        ConfigureNotifyClassification::ExternalAuthoritative
                    } else {
                        ConfigureNotifyClassification::UnknownPreserved
                    },
                    epoch: None,
                    geometry,
                    sequence_geometry_conflict: false,
                    sequence_wrap_progress: false,
                }
            }
        }
    }

    pub(crate) fn desired(&self) -> X11Geometry {
        self.desired
    }

    pub(crate) fn acknowledged(&self) -> Option<X11Geometry> {
        self.acknowledged
    }

    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub(crate) fn retired_len(&self) -> usize {
        self.retired.len()
    }

    fn pending_match(&self, geometry: X11Geometry, sequence: Option<u16>) -> PendingMatch {
        let mut first_geometry_match = None;
        let mut newest_geometry_match = None;
        let mut geometry_match_count = 0;
        for (index, expected) in self.pending.iter().enumerate() {
            if expected.geometry == geometry {
                first_geometry_match.get_or_insert(index);
                newest_geometry_match = Some(index);
                geometry_match_count += 1;
            }
        }

        let sequence_geometry_conflict = sequence.is_some_and(|sequence| {
            self.pending.iter().any(|expected| {
                expected.geometry != geometry
                    && expected
                        .configure_cookie_sequence
                        .is_some_and(|cookie| sequence16_eq(cookie, sequence))
            })
        });

        let Some(first_geometry_match) = first_geometry_match else {
            return if sequence.is_some_and(|sequence| {
                self.pending.iter().any(|expected| {
                    expected
                        .configure_cookie_sequence
                        .is_some_and(|cookie| sequence16_eq(cookie, sequence))
                })
            }) {
                PendingMatch::SequenceOnlyRejected
            } else {
                PendingMatch::None
            };
        };

        if geometry_match_count == 1 {
            let expected = self
                .pending
                .get(first_geometry_match)
                .expect("geometry match index was found");
            return PendingMatch::Matched {
                index: first_geometry_match,
                sequence_geometry_conflict,
                sequence_wrap_progress: sequence.is_some_and(|sequence| {
                    expected
                        .configure_cookie_sequence
                        .is_some_and(|cookie| sequence_progress_wrapped(sequence, cookie))
                }),
            };
        }

        let newest_geometry_match = newest_geometry_match.expect("geometry match was found");
        let newest = self
            .pending
            .get(newest_geometry_match)
            .expect("newest geometry match index was found");
        let can_prove_newest = sequence.is_some_and(|sequence| {
            newest
                .configure_cookie_sequence
                .is_some_and(|cookie| sequence_progress_reaches(sequence, cookie))
        });
        let newer_different_geometry_reached = sequence.is_some_and(|sequence| {
            self.pending
                .iter()
                .skip(newest_geometry_match + 1)
                .any(|expected| {
                    expected.geometry != geometry
                        && expected
                            .configure_cookie_sequence
                            .is_some_and(|cookie| sequence_progress_reaches(sequence, cookie))
                })
        });

        if can_prove_newest && !newer_different_geometry_reached {
            return PendingMatch::Matched {
                index: newest_geometry_match,
                sequence_geometry_conflict,
                sequence_wrap_progress: sequence.is_some_and(|sequence| {
                    newest
                        .configure_cookie_sequence
                        .is_some_and(|cookie| sequence_progress_wrapped(sequence, cookie))
                }),
            };
        }

        PendingMatch::Ambiguous {
            sequence_geometry_conflict,
        }
    }

    fn trim_pending(&mut self) {
        while self.pending.len() > CONFIGURE_HISTORY_LIMIT {
            let retired = self.pending.pop_front().expect("pending configure exists");
            self.retire(retired);
        }
    }

    fn retire(&mut self, expected: ExpectedConfigure) {
        self.retired.push_back(RetiredConfigure {
            epoch: expected.epoch,
            geometry: expected.geometry,
            source: expected.source,
            configure_cookie_sequence: expected.configure_cookie_sequence,
        });
        while self.retired.len() > CONFIGURE_HISTORY_LIMIT {
            self.retired.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(x: i32, width: u32) -> X11Geometry {
        X11Geometry {
            x,
            y: 100,
            width,
            height: 480,
        }
    }

    #[test]
    fn rapid_configures_keep_newest_desired_geometry_when_notifications_arrive_oldest_first() {
        let mut timeline = WindowConfigureTimeline::new(geometry(100, 640));
        let values = [geometry(110, 630), geometry(120, 620), geometry(130, 610)];
        for value in values {
            timeline.record(
                value,
                X11ConfigureFlags::all(),
                ConfigureSource::ClientRequest,
                None,
            );
        }

        let result = timeline.notify(values[0], None, false);

        assert_eq!(
            result.classification,
            ConfigureNotifyClassification::ExpectedOlder
        );
        assert_eq!(timeline.desired(), values[2]);
        assert_eq!(timeline.acknowledged(), Some(values[0]));
        assert_eq!(timeline.pending_len(), 2);
    }

    #[test]
    fn every_rapid_notification_order_preserves_the_newest_desired_geometry() {
        let values = [geometry(110, 630), geometry(120, 620), geometry(130, 610)];
        let orders = [
            vec![0, 1, 2],
            vec![2],
            vec![0, 2],
            vec![1, 0, 2],
            vec![2, 0],
        ];

        for order in orders {
            let mut timeline = WindowConfigureTimeline::new(geometry(100, 640));
            for value in values {
                timeline.record(
                    value,
                    X11ConfigureFlags::all(),
                    ConfigureSource::ClientRequest,
                    None,
                );
            }
            for index in order {
                timeline.notify(values[index], None, false);
            }
            assert_eq!(timeline.desired(), values[2]);
        }
    }

    #[test]
    fn rapid_top_left_notifications_keep_the_latest_complete_box_atomic() {
        let values = [
            X11Geometry {
                x: 110,
                y: 90,
                width: 630,
                height: 490,
            },
            X11Geometry {
                x: 120,
                y: 80,
                width: 620,
                height: 500,
            },
            X11Geometry {
                x: 130,
                y: 70,
                width: 610,
                height: 510,
            },
        ];
        let mut timeline = WindowConfigureTimeline::new(X11Geometry {
            x: 100,
            y: 100,
            width: 640,
            height: 480,
        });
        for value in values {
            timeline.record(
                value,
                X11ConfigureFlags::all(),
                ConfigureSource::ClientRequest,
                None,
            );
        }

        timeline.notify(values[0], None, false);

        assert_eq!(timeline.desired(), values[2]);
        assert_eq!(timeline.desired().x + timeline.desired().width as i32, 740);
        assert_eq!(timeline.desired().y + timeline.desired().height as i32, 580);
    }

    #[test]
    fn newest_notification_retires_older_configures_and_late_events_are_stale() {
        let mut timeline = WindowConfigureTimeline::new(geometry(100, 640));
        let values = [geometry(110, 630), geometry(120, 620), geometry(130, 610)];
        for value in values {
            timeline.record(
                value,
                X11ConfigureFlags::all(),
                ConfigureSource::Compositor,
                None,
            );
        }

        let current = timeline.notify(values[2], None, false);
        let stale = timeline.notify(values[0], None, false);

        assert_eq!(
            current.classification,
            ConfigureNotifyClassification::ExpectedCoalesced
        );
        assert_eq!(
            stale.classification,
            ConfigureNotifyClassification::StaleRetired
        );
        assert_eq!(timeline.desired(), values[2]);
        assert_eq!(timeline.pending_len(), 0);
        assert_eq!(timeline.retired_len(), 2);
    }

    #[test]
    fn contradictory_sequence_cannot_override_geometry_identity() {
        let mut timeline = WindowConfigureTimeline::new(geometry(100, 640));
        let first = timeline.record(
            geometry(110, 630),
            X11ConfigureFlags::all(),
            ConfigureSource::Compositor,
            Some(0x10001),
        );
        let second = timeline.record(
            geometry(120, 620),
            X11ConfigureFlags::all(),
            ConfigureSource::Compositor,
            Some(0x10002),
        );

        let result = timeline.notify(first.geometry, Some(2), false);

        assert_eq!(result.epoch, Some(first.epoch));
        assert_eq!(timeline.acknowledged(), Some(first.geometry));
        assert_ne!(timeline.acknowledged(), Some(second.geometry));
    }

    #[test]
    fn sequence_only_match_is_rejected_without_acknowledging_geometry() {
        let mut timeline = WindowConfigureTimeline::new(geometry(100, 640));
        let expected = timeline.record(
            geometry(110, 630),
            X11ConfigureFlags::all(),
            ConfigureSource::Compositor,
            Some(10),
        );

        let result = timeline.notify(geometry(999, 1), Some(10), false);

        assert_eq!(timeline.acknowledged(), None);
        assert_eq!(timeline.desired(), expected.geometry);
        assert_eq!(timeline.pending_len(), 1);
        assert_ne!(result.epoch, Some(expected.epoch));
    }

    #[test]
    fn unrelated_progress_sequence_cannot_acknowledge_a_different_geometry() {
        let mut timeline = WindowConfigureTimeline::new(geometry(100, 640));
        let first = timeline.record(
            geometry(110, 630),
            X11ConfigureFlags::all(),
            ConfigureSource::Compositor,
            Some(10),
        );
        let second = timeline.record(
            geometry(120, 620),
            X11ConfigureFlags::all(),
            ConfigureSource::Compositor,
            Some(11),
        );

        let result = timeline.notify(first.geometry, Some(12), false);

        assert_eq!(result.epoch, Some(first.epoch));
        assert_eq!(timeline.acknowledged(), Some(first.geometry));
        assert_ne!(timeline.acknowledged(), Some(second.geometry));
        assert_eq!(timeline.pending_len(), 1);
    }

    #[test]
    fn contradictory_progress_keeps_repeated_geometry_ambiguous() {
        let mut timeline = WindowConfigureTimeline::new(geometry(100, 640));
        let repeated = geometry(110, 630);
        timeline.record(
            repeated,
            X11ConfigureFlags::all(),
            ConfigureSource::Compositor,
            Some(1),
        );
        let different = timeline.record(
            geometry(120, 620),
            X11ConfigureFlags::all(),
            ConfigureSource::Compositor,
            Some(2),
        );
        timeline.record(
            repeated,
            X11ConfigureFlags::all(),
            ConfigureSource::Compositor,
            Some(3),
        );

        let result = timeline.notify(repeated, Some(2), false);

        assert_eq!(timeline.acknowledged(), None);
        assert_eq!(timeline.pending_len(), 3);
        assert_ne!(result.epoch, Some(different.epoch));
    }

    #[test]
    fn wraparound_progress_can_disambiguate_the_newest_identical_geometry() {
        let mut timeline = WindowConfigureTimeline::new(geometry(100, 640));
        let repeated = geometry(110, 630);
        timeline.record(
            repeated,
            X11ConfigureFlags::all(),
            ConfigureSource::Compositor,
            Some(0xfffe),
        );
        timeline.record(
            geometry(120, 620),
            X11ConfigureFlags::all(),
            ConfigureSource::Compositor,
            Some(0xffff),
        );
        let newest = timeline.record(
            repeated,
            X11ConfigureFlags::all(),
            ConfigureSource::Compositor,
            Some(0x1_0001),
        );

        let result = timeline.notify(repeated, Some(2), false);

        assert_eq!(result.epoch, Some(newest.epoch));
        assert_eq!(timeline.acknowledged(), Some(repeated));
        assert_eq!(timeline.pending_len(), 0);
        assert_eq!(timeline.retired_len(), 2);
    }

    #[test]
    fn sequence16_equality_uses_only_the_event_width() {
        assert!(sequence16_eq(0x1_0001, 0x0001));
        assert!(!sequence16_eq(0x1_0001, 0x0000));
    }

    #[test]
    fn sequence16_forward_progress_wraps_at_zero_and_rejects_stale_progress() {
        assert!(sequence16_is_after(0xffff, 0xfffe));
        assert!(sequence16_is_after(0x0000, 0xffff));
        assert!(sequence16_is_after(0x0001, 0x0000));
        assert!(!sequence16_is_after(0xffff, 0x0000));
        assert!(!sequence16_is_after(0x8000, 0x0000));
    }

    #[test]
    fn geometry_match_records_wrapped_notification_progress() {
        let mut timeline = WindowConfigureTimeline::new(geometry(100, 640));
        let expected_geometry = geometry(110, 630);
        timeline.record(
            expected_geometry,
            X11ConfigureFlags::all(),
            ConfigureSource::Compositor,
            Some(0xffff),
        );

        let result = timeline.notify(expected_geometry, Some(0), false);

        assert!(result.sequence_wrap_progress);
    }

    #[test]
    fn client_authoritative_window_can_reuse_retired_geometry() {
        let mut timeline = WindowConfigureTimeline::new(geometry(100, 640));
        let retired_geometry = geometry(110, 630);
        timeline.record(
            retired_geometry,
            X11ConfigureFlags::all(),
            ConfigureSource::Compositor,
            Some(1),
        );
        timeline.record(
            geometry(120, 620),
            X11ConfigureFlags::all(),
            ConfigureSource::Compositor,
            Some(2),
        );
        timeline.notify(geometry(120, 620), Some(2), false);

        let result = timeline.notify(retired_geometry, None, true);

        assert_ne!(
            result.classification,
            ConfigureNotifyClassification::StaleRetired
        );
        assert_eq!(result.geometry, retired_geometry);
    }

    #[test]
    fn client_authoritative_retired_geometry_with_matching_cookie_is_stale_self_configure() {
        let mut timeline = WindowConfigureTimeline::new(geometry(100, 640));
        let retired_geometry = geometry(110, 630);
        timeline.record(
            retired_geometry,
            X11ConfigureFlags::all(),
            ConfigureSource::Compositor,
            Some(1),
        );
        timeline.record(
            geometry(120, 620),
            X11ConfigureFlags::all(),
            ConfigureSource::Compositor,
            Some(2),
        );
        timeline.notify(geometry(120, 620), Some(2), false);

        let result = timeline.notify(retired_geometry, Some(1), true);

        assert_eq!(
            result.classification,
            ConfigureNotifyClassification::StaleRetired
        );
    }

    #[test]
    fn incoming_client_sequence_is_not_stored_as_outgoing_cookie_sequence() {
        let mut timeline = WindowConfigureTimeline::new(geometry(100, 640));
        let client_event_sequence = 77_u16;
        let configure_cookie_sequence = 0x1_0000_004d_u64;
        let expected = timeline.record(
            geometry(110, 630),
            X11ConfigureFlags::all(),
            ConfigureSource::ClientRequest,
            Some(configure_cookie_sequence),
        );

        assert_eq!(
            expected.configure_cookie_sequence,
            Some(configure_cookie_sequence)
        );
        assert_ne!(
            expected.configure_cookie_sequence,
            Some(u64::from(client_event_sequence))
        );
    }

    #[test]
    fn history_is_bounded() {
        let mut timeline = WindowConfigureTimeline::new(geometry(100, 640));
        for x in 0..(CONFIGURE_HISTORY_LIMIT * 3) {
            timeline.record(
                geometry(x as i32, 640),
                X11ConfigureFlags::all(),
                ConfigureSource::Compositor,
                None,
            );
        }

        assert!(timeline.pending_len() <= CONFIGURE_HISTORY_LIMIT);
        assert!(timeline.retired_len() <= CONFIGURE_HISTORY_LIMIT);
    }
}
