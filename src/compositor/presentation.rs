use std::io;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PresentationClock {
    #[default]
    Monotonic,
    Realtime,
}

impl PresentationClock {
    pub const fn clock_id(self) -> libc::clockid_t {
        match self {
            Self::Monotonic => libc::CLOCK_MONOTONIC,
            Self::Realtime => libc::CLOCK_REALTIME,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationTimestamp {
    seconds: u64,
    nanoseconds: u32,
}

impl PresentationTimestamp {
    pub fn from_microseconds(seconds: u64, microseconds: u32) -> io::Result<Self> {
        if microseconds >= 1_000_000 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("presentation microseconds out of range: {microseconds}"),
            ));
        }
        Ok(Self {
            seconds,
            nanoseconds: microseconds * 1_000,
        })
    }

    pub fn from_clock(clock: PresentationClock) -> io::Result<Self> {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if unsafe { libc::clock_gettime(clock.clock_id(), &mut time) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let seconds = u64::try_from(time.tv_sec).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "presentation clock returned negative seconds: {}",
                    time.tv_sec
                ),
            )
        })?;
        let nanoseconds = u32::try_from(time.tv_nsec)
            .ok()
            .filter(|nanoseconds| *nanoseconds < 1_000_000_000)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "presentation clock returned invalid nanoseconds: {}",
                        time.tv_nsec
                    ),
                )
            })?;
        Ok(Self {
            seconds,
            nanoseconds,
        })
    }

    pub const fn protocol_seconds(self) -> (u32, u32) {
        ((self.seconds >> 32) as u32, self.seconds as u32)
    }

    pub const fn nanoseconds(self) -> u32 {
        self.nanoseconds
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationKind {
    Synchronized,
    Software,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FramePresentation {
    pub clock: PresentationClock,
    pub timestamp: PresentationTimestamp,
    pub sequence: u64,
    pub kind: PresentationKind,
    pub zero_copy: bool,
}

impl FramePresentation {
    pub fn synchronized(
        clock: PresentationClock,
        seconds: u32,
        microseconds: u32,
        sequence: u32,
    ) -> io::Result<Self> {
        Ok(Self {
            clock,
            timestamp: PresentationTimestamp::from_microseconds(u64::from(seconds), microseconds)?,
            sequence: u64::from(sequence),
            kind: PresentationKind::Synchronized,
            zero_copy: false,
        })
    }

    pub fn synchronized_zero_copy(
        clock: PresentationClock,
        seconds: u32,
        microseconds: u32,
        sequence: u32,
    ) -> io::Result<Self> {
        let mut presentation = Self::synchronized(clock, seconds, microseconds, sequence)?;
        presentation.zero_copy = true;
        Ok(presentation)
    }

    pub fn software_now(clock: PresentationClock) -> io::Result<Self> {
        Ok(Self {
            clock,
            timestamp: PresentationTimestamp::from_clock(clock)?,
            sequence: 0,
            kind: PresentationKind::Software,
            zero_copy: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microseconds_convert_exactly_to_nanoseconds() {
        let timestamp = PresentationTimestamp::from_microseconds(7, 123_456).unwrap();

        assert_eq!(timestamp.nanoseconds(), 123_456_000);
    }

    #[test]
    fn last_valid_microsecond_preserves_protocol_precision() {
        let timestamp = PresentationTimestamp::from_microseconds(7, 999_999).unwrap();

        assert_eq!(timestamp.nanoseconds(), 999_999_000);
    }

    #[test]
    fn invalid_microsecond_is_rejected() {
        assert!(PresentationTimestamp::from_microseconds(7, 1_000_000).is_err());
    }

    #[test]
    fn seconds_split_into_protocol_high_and_low_words() {
        let timestamp = PresentationTimestamp::from_microseconds(0x1234_5678_9abc_def0, 0).unwrap();

        assert_eq!(timestamp.protocol_seconds(), (0x1234_5678, 0x9abc_def0));
    }

    #[test]
    fn maximum_seconds_split_without_overflow() {
        let timestamp = PresentationTimestamp::from_microseconds(u64::MAX, 0).unwrap();

        assert_eq!(timestamp.protocol_seconds(), (u32::MAX, u32::MAX));
    }

    #[test]
    fn direct_presentation_is_zero_copy_but_composited_presentation_is_not() {
        let direct =
            FramePresentation::synchronized_zero_copy(PresentationClock::Monotonic, 1, 0, 1)
                .unwrap();
        let composited =
            FramePresentation::synchronized(PresentationClock::Monotonic, 1, 0, 2).unwrap();

        assert!(direct.zero_copy);
        assert!(!composited.zero_copy);
    }

    #[test]
    fn zero_microseconds_remain_zero_nanoseconds() {
        let timestamp = PresentationTimestamp::from_microseconds(7, 0).unwrap();

        assert_eq!(timestamp.nanoseconds(), 0);
    }
}
