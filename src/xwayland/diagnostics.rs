//! Bounded diagnostics for the active and most recently failed XWayland
//! generation.

use std::collections::VecDeque;

pub(crate) const STDERR_RING_LINES: usize = 256;
pub(crate) const STDERR_LINE_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum XwaylandExitClass {
    ExpectedShutdownAfterRunning,
    ExpectedIdleExitAfterRunning,
    StartupExitBeforeReadiness,
    CrashOrSignal,
    CompositorRequestedTermination,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StderrLine {
    pub(crate) text: String,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct StderrRing {
    lines: VecDeque<StderrLine>,
}

impl StderrRing {
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        let mut start = 0;
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' {
                self.push_line(&bytes[start..index]);
                start = index.saturating_add(1);
            }
        }
        if start < bytes.len() {
            self.push_line(&bytes[start..]);
        }
    }

    pub(crate) fn push_line(&mut self, bytes: &[u8]) {
        let truncated = bytes.len() > STDERR_LINE_BYTES;
        let end = bytes.len().min(STDERR_LINE_BYTES);
        self.lines.push_back(StderrLine {
            text: String::from_utf8_lossy(&bytes[..end]).into_owned(),
            truncated,
        });
        while self.lines.len() > STDERR_RING_LINES {
            self.lines.pop_front();
        }
    }

    pub(crate) fn lines(&self) -> impl Iterator<Item = &StderrLine> {
        self.lines.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_ring_is_bounded_and_marks_long_lines() {
        let mut ring = StderrRing::default();
        for _ in 0..(STDERR_RING_LINES + 3) {
            ring.push_line(b"line");
        }
        assert_eq!(ring.lines.len(), STDERR_RING_LINES);

        let mut ring = StderrRing::default();
        ring.push_line(&vec![b'x'; STDERR_LINE_BYTES + 1]);
        let line = ring.lines().next().expect("line retained");
        assert!(line.truncated);
        assert_eq!(line.text.len(), STDERR_LINE_BYTES);
    }

    #[test]
    fn stderr_ring_splits_newline_delimited_input() {
        let mut ring = StderrRing::default();
        ring.push(b"first\nsecond\n");
        let lines = ring
            .lines()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(lines, ["first", "second"]);
    }
}
