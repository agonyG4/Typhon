//! Holds secondary-only frames for the fullscreen cadence owner (VRR Phase 4).
//!
//! Under VRR every composited frame starts a refresh. A shell overlay above a
//! fullscreen game (spotlight, notification) animating at the panel maximum
//! interleaves its own refreshes with the game's frames: max-refresh spikes in
//! a stable 60-90 Hz cadence. While only secondary work changed, the frame
//! waits for the owner's next expected frame and rides along with it; when
//! the owner is late, the secondary work presents on its own.

/// Owner intervals kept for the cadence estimate.
const OWNER_INTERVALS: usize = 8;
/// Intervals needed before the owner's cadence is trusted.
const MIN_OWNER_INTERVALS: usize = 4;
/// Owner intervals longer than this are pauses, not cadence.
const MAX_OWNER_INTERVAL_NS: u64 = 100_000_000;
/// Margin past the owner's expected frame before secondary work goes alone.
const HOLD_SLACK_NS: u64 = 2_000_000;
/// Longest a secondary frame waits after the owner's last frame.
const MAX_HOLD_NS: u64 = 50_000_000;

/// `OBLIVION_ONE_VRR_SECONDARY_HOLD=off` presents secondary work at once,
/// for an A/B against the hold.
pub(crate) fn secondary_hold_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("OBLIVION_ONE_VRR_SECONDARY_HOLD").map_or(true, |value| value != "off")
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SecondaryHoldRelease {
    /// The owner presented new content; the secondary work rides with it.
    Owner,
    /// The owner missed its expected frame; the secondary work goes alone.
    Deadline,
}

impl SecondaryHoldRelease {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Deadline => "deadline",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SecondaryFrameHold {
    owner_root: Option<u32>,
    /// Latest owner content commit observed.
    owner_sequence: u64,
    /// Owner content commit included in the latest admitted repaint.
    admitted_sequence: u64,
    last_owner_change_ns: Option<u64>,
    intervals: [u64; OWNER_INTERVALS],
    interval_count: usize,
    next_interval: usize,
    held_until_ns: Option<u64>,
}

impl SecondaryFrameHold {
    /// Feeds the fullscreen cadence owner's root and newest content commit
    /// sequence (None without a cadence owner).
    pub(crate) fn observe_owner(&mut self, now_ns: u64, owner: Option<(u32, u64)>) {
        let Some((root, sequence)) = owner else {
            *self = Self::default();
            return;
        };
        if self.owner_root != Some(root) {
            *self = Self {
                owner_root: Some(root),
                owner_sequence: sequence,
                admitted_sequence: sequence,
                last_owner_change_ns: Some(now_ns),
                ..Self::default()
            };
            return;
        }
        if sequence == self.owner_sequence {
            return;
        }
        self.owner_sequence = sequence;
        if let Some(previous) = self.last_owner_change_ns.replace(now_ns) {
            let interval = now_ns.saturating_sub(previous);
            if interval == 0 || interval > MAX_OWNER_INTERVAL_NS {
                self.interval_count = 0;
                self.next_interval = 0;
            } else {
                self.intervals[self.next_interval] = interval;
                self.next_interval = (self.next_interval + 1) % OWNER_INTERVALS;
                self.interval_count = (self.interval_count + 1).min(OWNER_INTERVALS);
            }
        }
    }

    fn owner_interval_estimate_ns(&self) -> Option<u64> {
        if self.interval_count < MIN_OWNER_INTERVALS {
            return None;
        }
        let mut sorted = self.intervals;
        let sorted = &mut sorted[..self.interval_count];
        sorted.sort_unstable();
        Some(sorted[sorted.len() / 2])
    }

    /// Decides a pending repaint. Returns the time to hold it until when only
    /// secondary work changed and the owner's next frame is still expected.
    pub(crate) fn hold_until_ns(&mut self, now_ns: u64, eligible: bool) -> Option<u64> {
        let hold = (|| {
            if !eligible || self.owner_sequence != self.admitted_sequence {
                return None;
            }
            let last_owner_change_ns = self.last_owner_change_ns?;
            let estimate = self.owner_interval_estimate_ns()?;
            let until = last_owner_change_ns
                .saturating_add(estimate.saturating_add(HOLD_SLACK_NS).min(MAX_HOLD_NS));
            (now_ns < until).then_some(until)
        })();
        if hold.is_some() {
            self.held_until_ns = hold;
        }
        hold
    }

    /// Records an admitted repaint; returns how a hold, if any, ended.
    pub(crate) fn note_admitted(&mut self) -> Option<SecondaryHoldRelease> {
        let release = self.held_until_ns.take().map(|_| {
            if self.owner_sequence != self.admitted_sequence {
                SecondaryHoldRelease::Owner
            } else {
                SecondaryHoldRelease::Deadline
            }
        });
        self.admitted_sequence = self.owner_sequence;
        release
    }

    /// Wake deadline while a repaint is held.
    pub(crate) fn wake_deadline_ns(&self, now_ns: u64) -> Option<u64> {
        self.held_until_ns.filter(|until| *until > now_ns)
    }

    pub(crate) const fn held(&self) -> bool {
        self.held_until_ns.is_some()
    }

    pub(crate) fn owner_interval_ns(&self) -> u64 {
        self.owner_interval_estimate_ns().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;
    const FRAME_60: u64 = 16_666_667;

    /// A 60 fps owner with `frames` content commits, starting at `start`.
    fn owner_at_60(hold: &mut SecondaryFrameHold, start: u64, frames: u64) -> u64 {
        let mut now = start;
        for sequence in 1..=frames {
            hold.observe_owner(now, Some((7, sequence)));
            assert_eq!(hold.hold_until_ns(now, true), None);
            hold.note_admitted();
            now += FRAME_60;
        }
        now - FRAME_60
    }

    #[test]
    fn secondary_only_damage_waits_for_the_owner_and_rides_its_frame() {
        let mut hold = SecondaryFrameHold::default();
        let last = owner_at_60(&mut hold, 1_000 * MS, 8);
        // An overlay frame 4 ms after the owner's frame is held until the
        // owner's next expected frame plus slack.
        let overlay = last + 4 * MS;
        hold.observe_owner(overlay, Some((7, 8)));
        let until = hold.hold_until_ns(overlay, true).expect("held");
        assert_eq!(until, last + FRAME_60 + HOLD_SLACK_NS);
        assert_eq!(hold.wake_deadline_ns(overlay), Some(until));
        // The owner commits on time: the repaint goes out with both.
        let next = last + FRAME_60;
        hold.observe_owner(next, Some((7, 9)));
        assert_eq!(hold.hold_until_ns(next, true), None);
        assert_eq!(hold.note_admitted(), Some(SecondaryHoldRelease::Owner));
        assert!(!hold.held());
    }

    #[test]
    fn a_late_owner_releases_the_secondary_frame_at_the_deadline() {
        let mut hold = SecondaryFrameHold::default();
        let last = owner_at_60(&mut hold, 1_000 * MS, 8);
        let until = hold.hold_until_ns(last + 4 * MS, true).expect("held");
        assert_eq!(hold.hold_until_ns(until, true), None);
        assert_eq!(hold.note_admitted(), Some(SecondaryHoldRelease::Deadline));
        // A paused owner never holds again until it resumes its cadence.
        assert_eq!(hold.hold_until_ns(until + 30 * MS, true), None);
    }

    #[test]
    fn no_hold_without_a_trusted_owner_cadence_or_eligibility() {
        let mut hold = SecondaryFrameHold::default();
        let last = owner_at_60(&mut hold, 1_000 * MS, 3);
        assert_eq!(hold.hold_until_ns(last + 4 * MS, true), None);
        let mut hold = SecondaryFrameHold::default();
        let last = owner_at_60(&mut hold, 1_000 * MS, 8);
        assert_eq!(hold.hold_until_ns(last + 4 * MS, false), None);
        // Losing the cadence owner forgets everything.
        hold.observe_owner(last + 5 * MS, None);
        assert_eq!(hold.hold_until_ns(last + 6 * MS, true), None);
    }

    #[test]
    fn a_slow_owner_holds_at_most_the_cap() {
        let mut hold = SecondaryFrameHold::default();
        let mut now = 1_000 * MS;
        for sequence in 1..=8 {
            hold.observe_owner(now, Some((7, sequence)));
            hold.note_admitted();
            now += 80 * MS;
        }
        let last = now - 80 * MS;
        assert_eq!(
            hold.hold_until_ns(last + MS, true),
            Some(last + MAX_HOLD_NS)
        );
    }

    #[test]
    fn a_new_owner_starts_a_fresh_estimate() {
        let mut hold = SecondaryFrameHold::default();
        let last = owner_at_60(&mut hold, 1_000 * MS, 8);
        hold.observe_owner(last + MS, Some((9, 100)));
        assert_eq!(hold.hold_until_ns(last + 2 * MS, true), None);
    }
}
