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
    pub(crate) x11_request_sequence: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RetiredConfigure {
    pub(crate) epoch: u64,
    pub(crate) geometry: X11Geometry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfigureNotifyClassification {
    ExpectedCurrent,
    ExpectedOlder,
    ExpectedCoalesced,
    StaleRetired,
    ExternalAuthoritative,
    UnknownPreserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConfigureNotifyResult {
    pub(crate) classification: ConfigureNotifyClassification,
    pub(crate) epoch: Option<u64>,
    pub(crate) geometry: X11Geometry,
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
        x11_request_sequence: Option<u64>,
    ) -> ExpectedConfigure {
        self.next_epoch = self.next_epoch.saturating_add(1).max(1);
        let expected = ExpectedConfigure {
            epoch: self.next_epoch,
            geometry,
            fields,
            source,
            x11_request_sequence,
        };
        self.desired = geometry;
        self.pending.push_back(expected);
        self.trim_pending();
        expected
    }

    pub(crate) fn notify(
        &mut self,
        geometry: X11Geometry,
        x11_event_sequence: Option<u64>,
        external_authoritative: bool,
    ) -> ConfigureNotifyResult {
        if let Some(index) = self.pending_match(geometry, x11_event_sequence) {
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

            return ConfigureNotifyResult {
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
            };
        }

        if let Some(retired) = self
            .retired
            .iter()
            .find(|retired| retired.geometry == geometry)
        {
            return ConfigureNotifyResult {
                classification: ConfigureNotifyClassification::StaleRetired,
                epoch: Some(retired.epoch),
                geometry,
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

    fn pending_match(&self, geometry: X11Geometry, sequence: Option<u64>) -> Option<usize> {
        sequence
            .filter(|sequence| *sequence != 0)
            .and_then(|sequence| {
                self.pending.iter().position(|expected| {
                    expected
                        .x11_request_sequence
                        .is_some_and(|request| request as u16 == sequence as u16)
                })
            })
            .or_else(|| {
                self.pending
                    .iter()
                    .position(|expected| expected.geometry == geometry)
            })
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
    fn sequence_matches_the_expected_entry_before_geometry_fallback() {
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

        assert_eq!(result.epoch, Some(second.epoch));
        assert_eq!(timeline.acknowledged(), Some(second.geometry));
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
