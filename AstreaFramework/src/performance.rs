#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameBudget {
    refresh_rate_hz: u32,
    frame_time_micros: u32,
}

impl FrameBudget {
    pub fn from_refresh_rate_hz(refresh_rate_hz: u32) -> Option<Self> {
        (refresh_rate_hz > 0).then_some(Self {
            refresh_rate_hz,
            frame_time_micros: 1_000_000 / refresh_rate_hz,
        })
    }

    pub const fn refresh_rate_hz(self) -> u32 {
        self.refresh_rate_hz
    }

    pub const fn frame_time_micros(self) -> u32 {
        self.frame_time_micros
    }

    pub const fn allows_effect_cost_micros(self, cost_micros: u32) -> bool {
        cost_micros < self.frame_time_micros
    }
}
