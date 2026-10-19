//! Detects reactor busy loops: long runs of cycles whose epoll wait returned
//! without sleeping. Profiles showed bursts of ~1 s at a full core with no
//! voluntary context switch, which also exhaust the RR budget. The union of
//! wake and continuation reasons over the run names what kept re-waking it.

/// A wait shorter than this did not really sleep.
const BLOCKED_SLEEP_THRESHOLD_NS: u64 = 50_000;
/// Consecutive non-sleeping cycles before a run is reported.
const BUSY_LOOP_REPORT_CYCLES: u32 = 300;
/// Bound on reports per session so a persistent loop cannot flood the log.
const MAX_BUSY_LOOP_REPORTS: u32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BusyLoopReport {
    pub(super) cycles: u32,
    pub(super) duration_ns: u64,
    pub(super) wake_reasons: u32,
    pub(super) continuation_reasons: u32,
}

#[derive(Debug, Default)]
pub(super) struct BusyLoopDetector {
    streak: u32,
    streak_start_ns: u64,
    wake_union: u32,
    continuation_union: u32,
    reported: bool,
    reports: u32,
}

impl BusyLoopDetector {
    /// Feed one reactor wake. Returns a report once per run, when the run
    /// first reaches the reporting length.
    pub(super) fn observe(
        &mut self,
        now_ns: u64,
        blocked_ns: u64,
        wake_reasons: u32,
        continuation_reasons: u32,
    ) -> Option<BusyLoopReport> {
        if blocked_ns >= BLOCKED_SLEEP_THRESHOLD_NS {
            self.streak = 0;
            self.reported = false;
            return None;
        }
        if self.streak == 0 {
            self.streak_start_ns = now_ns;
            self.wake_union = 0;
            self.continuation_union = 0;
        }
        self.streak = self.streak.saturating_add(1);
        self.wake_union |= wake_reasons;
        self.continuation_union |= continuation_reasons;
        if self.reported
            || self.streak < BUSY_LOOP_REPORT_CYCLES
            || self.reports >= MAX_BUSY_LOOP_REPORTS
        {
            return None;
        }
        self.reported = true;
        self.reports += 1;
        Some(BusyLoopReport {
            cycles: self.streak,
            duration_ns: now_ns.saturating_sub(self.streak_start_ns),
            wake_reasons: self.wake_union,
            continuation_reasons: self.continuation_union,
        })
    }
}

// Bit layouts mirror `WakeReasons` and `NativeContinuationReasons` in
// src/native/event_loop.rs.
const WAKE_REASON_NAMES: [&str; 23] = [
    "drm",
    "wayland_listener",
    "wayland_clients",
    "input",
    "timer",
    "explicit_sync_acquire",
    "child_signal",
    "seat",
    "output_render_fence",
    "xwayland_listen",
    "xwayland_display_ready",
    "xwayland_xwm",
    "xwayland_stderr",
    "kms_commit_worker",
    "control",
    "cursor_io_worker",
    "dmabuf_gpu_release",
    "runtime_continuation",
    "keyboard_persistence_worker",
    "output_configuration_persistence_worker",
    "controller",
    "system_action",
    "render_worker",
];
const CONTINUATION_REASON_NAMES: [&str; 10] = [
    "input_backlog",
    "astrea_publication",
    "commit_timing_planning",
    "xwayland",
    "control_timeout",
    "scene_visual_debt",
    "frame_scheduler",
    "controller_backlog",
    "process_launch_backlog",
    "screen_capture_backlog",
];

fn bit_names(bits: u32, names: &[&str]) -> String {
    (0..32)
        .filter(|bit| bits & (1 << bit) != 0)
        .map(|bit| names.get(bit as usize).copied().unwrap_or("unknown"))
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn wake_reason_names(bits: u32) -> String {
    bit_names(bits, &WAKE_REASON_NAMES)
}

pub(super) fn continuation_reason_names(bits: u32) -> String {
    bit_names(bits, &CONTINUATION_REASON_NAMES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_once_per_non_sleeping_run_with_reason_unions() {
        let mut detector = BusyLoopDetector::default();
        let mut reports = Vec::new();
        for cycle in 0..(BUSY_LOOP_REPORT_CYCLES + 50) {
            let wake = if cycle % 2 == 0 { 0b001 } else { 0b100 };
            if let Some(report) = detector.observe(u64::from(cycle) * 1_000, 2_000, wake, 0b10) {
                reports.push(report);
            }
        }
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].cycles, BUSY_LOOP_REPORT_CYCLES);
        assert_eq!(reports[0].wake_reasons, 0b101);
        assert_eq!(reports[0].continuation_reasons, 0b10);
        assert_eq!(
            reports[0].duration_ns,
            u64::from(BUSY_LOOP_REPORT_CYCLES - 1) * 1_000
        );
    }

    #[test]
    fn a_real_sleep_resets_the_run() {
        let mut detector = BusyLoopDetector::default();
        for cycle in 0..(BUSY_LOOP_REPORT_CYCLES - 1) {
            assert!(detector.observe(u64::from(cycle), 0, 1, 0).is_none());
        }
        assert!(
            detector
                .observe(0, BLOCKED_SLEEP_THRESHOLD_NS, 1, 0)
                .is_none()
        );
        for cycle in 0..(BUSY_LOOP_REPORT_CYCLES - 1) {
            assert!(detector.observe(u64::from(cycle), 0, 1, 0).is_none());
        }
        assert!(detector.observe(1, 0, 1, 0).is_some());
    }

    #[test]
    fn reason_names_follow_event_loop_bit_layout() {
        assert_eq!(
            wake_reason_names((1 << 2) | (1 << 17)),
            "wayland_clients,runtime_continuation"
        );
        assert_eq!(continuation_reason_names(1 << 5), "scene_visual_debt");
        assert_eq!(wake_reason_names(1 << 30), "unknown");
    }

    #[test]
    fn reports_are_bounded_per_session() {
        let mut detector = BusyLoopDetector::default();
        let mut reports = 0;
        for _ in 0..(MAX_BUSY_LOOP_REPORTS + 5) {
            for _ in 0..BUSY_LOOP_REPORT_CYCLES {
                reports += usize::from(detector.observe(0, 0, 1, 0).is_some());
            }
            detector.observe(0, BLOCKED_SLEEP_THRESHOLD_NS, 0, 0);
        }
        assert_eq!(reports, MAX_BUSY_LOOP_REPORTS as usize);
    }
}
