/// Absolute monotonic timestamp used for analytical presentation sampling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AnimationTime(u64);

impl AnimationTime {
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    pub fn monotonic_now() -> Option<Self> {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
            return None;
        }
        let seconds = u64::try_from(time.tv_sec).ok()?;
        let nanos = u64::try_from(time.tv_nsec).ok()?;
        seconds
            .checked_mul(1_000_000_000)?
            .checked_add(nanos)
            .map(Self)
    }

    pub(crate) fn elapsed_seconds(self, start: Self) -> f64 {
        self.0.saturating_sub(start.0) as f64 / 1_000_000_000.0
    }
}
