#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DurationMs(u32);

impl DurationMs {
    pub const ZERO: Self = Self(0);

    pub const fn new(milliseconds: u32) -> Self {
        Self(milliseconds)
    }

    pub const fn as_millis(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionSpeed {
    multiplier: f32,
    instant: bool,
}

impl MotionSpeed {
    pub const fn instant() -> Self {
        Self {
            multiplier: 1.0,
            instant: true,
        }
    }

    pub fn from_multiplier(multiplier: f32) -> Option<Self> {
        (multiplier.is_finite() && multiplier > 0.0).then_some(Self {
            multiplier,
            instant: false,
        })
    }

    pub fn scale_duration(self, duration: DurationMs) -> DurationMs {
        if self.instant || duration == DurationMs::ZERO {
            return DurationMs::ZERO;
        }

        let scaled = (duration.as_millis() as f32 / self.multiplier).round() as u32;
        DurationMs::new(scaled.max(1))
    }
}

impl Default for MotionSpeed {
    fn default() -> Self {
        Self {
            multiplier: 1.0,
            instant: false,
        }
    }
}
