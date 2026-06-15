use wayland_server::{Resource, WEnum, protocol::wl_output};

const DEFAULT_OUTPUT_WIDTH: u32 = 1280;
const DEFAULT_OUTPUT_HEIGHT: u32 = 800;
const WL_OUTPUT_DONE_SINCE: u32 = 2;
const WL_OUTPUT_SCALE_SINCE: u32 = 2;
const WL_OUTPUT_NAME_SINCE: u32 = 4;
const WL_OUTPUT_DESCRIPTION_SINCE: u32 = 4;
const FRACTIONAL_SCALE_DENOMINATOR: u32 = 120;
const DEFAULT_OUTPUT_REFRESH_HZ: u32 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OutputSize {
    pub(super) width: u32,
    pub(super) height: u32,
}

impl OutputSize {
    pub(super) const fn new(width: u32, height: u32) -> Self {
        Self {
            width: if width == 0 { 1 } else { width },
            height: if height == 0 { 1 } else { height },
        }
    }
}

impl Default for OutputSize {
    fn default() -> Self {
        Self::new(DEFAULT_OUTPUT_WIDTH, DEFAULT_OUTPUT_HEIGHT)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OutputScale {
    preferred_scale: u32,
}

impl OutputScale {
    pub(super) fn from_factor(scale_factor: f64) -> Self {
        let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };
        Self {
            preferred_scale: (scale_factor * f64::from(FRACTIONAL_SCALE_DENOMINATOR))
                .round()
                .max(1.0) as u32,
        }
    }

    pub(super) const fn preferred_scale(self) -> u32 {
        self.preferred_scale
    }

    pub(super) fn wl_output_scale(self) -> i32 {
        self.preferred_scale
            .div_ceil(FRACTIONAL_SCALE_DENOMINATOR)
            .max(1) as i32
    }
}

impl Default for OutputScale {
    fn default() -> Self {
        Self::from_factor(1.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct OutputRefreshRate {
    refresh_hz: u32,
}

impl OutputRefreshRate {
    pub(super) fn from_hz(refresh_hz: u32) -> Self {
        Self {
            refresh_hz: normalize_refresh_hz(refresh_hz),
        }
    }

    pub(super) fn wl_output_millihertz(self) -> i32 {
        self.refresh_hz.saturating_mul(1_000) as i32
    }

    pub(super) fn presentation_refresh_nsec(self) -> u32 {
        1_000_000_000 / self.refresh_hz.max(1)
    }
}

impl Default for OutputRefreshRate {
    fn default() -> Self {
        Self::from_hz(DEFAULT_OUTPUT_REFRESH_HZ)
    }
}

fn normalize_refresh_hz(refresh_hz: u32) -> u32 {
    if refresh_hz == 0 {
        DEFAULT_OUTPUT_REFRESH_HZ
    } else {
        refresh_hz.clamp(24, 1_000)
    }
}

pub(super) fn send_output_description(
    output: &wl_output::WlOutput,
    output_size: OutputSize,
    output_scale: OutputScale,
    output_refresh: OutputRefreshRate,
) {
    let _ = output.send_event(wl_output::Event::Geometry {
        x: 0,
        y: 0,
        physical_width: 344,
        physical_height: 215,
        subpixel: WEnum::Value(wl_output::Subpixel::Unknown),
        make: "Oblivion".to_string(),
        model: "Nested Output".to_string(),
        transform: WEnum::Value(wl_output::Transform::Normal),
    });
    send_output_mode(output, output_size, output_refresh);
    send_output_scale(output, output_scale);
    if output.version() >= WL_OUTPUT_NAME_SINCE {
        let _ = output.send_event(wl_output::Event::Name {
            name: "Oblivion-1".to_string(),
        });
    }
    if output.version() >= WL_OUTPUT_DESCRIPTION_SINCE {
        let _ = output.send_event(wl_output::Event::Description {
            description: "Oblivion One nested output".to_string(),
        });
    }
    send_output_done_if_supported(output);
}

pub(super) fn send_output_scale(output: &wl_output::WlOutput, output_scale: OutputScale) {
    if output.version() >= WL_OUTPUT_SCALE_SINCE {
        let _ = output.send_event(wl_output::Event::Scale {
            factor: output_scale.wl_output_scale(),
        });
    }
}

pub(super) fn send_output_mode(
    output: &wl_output::WlOutput,
    output_size: OutputSize,
    output_refresh: OutputRefreshRate,
) {
    let _ = output.send_event(wl_output::Event::Mode {
        flags: WEnum::Value(wl_output::Mode::Current | wl_output::Mode::Preferred),
        width: output_size.width as i32,
        height: output_size.height as i32,
        refresh: output_refresh.wl_output_millihertz(),
    });
}

pub(super) fn send_output_done_if_supported(output: &wl_output::WlOutput) {
    if output.version() >= WL_OUTPUT_DONE_SINCE {
        let _ = output.send_event(wl_output::Event::Done);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_refresh_rate_converts_hz_to_wayland_millihertz() {
        assert_eq!(
            OutputRefreshRate::from_hz(165).wl_output_millihertz(),
            165_000
        );
    }

    #[test]
    fn output_refresh_rate_converts_hz_to_presentation_nsec() {
        assert_eq!(
            OutputRefreshRate::from_hz(165).presentation_refresh_nsec(),
            6_060_606
        );
    }

    #[test]
    fn output_refresh_rate_falls_back_to_sixty_hz_when_missing() {
        let refresh = OutputRefreshRate::from_hz(0);

        assert_eq!(refresh.wl_output_millihertz(), 60_000);
        assert_eq!(refresh.presentation_refresh_nsec(), 16_666_666);
    }

    #[test]
    fn output_refresh_rate_accepts_nested_cli_range() {
        let refresh = OutputRefreshRate::from_hz(1_000);

        assert_eq!(refresh.wl_output_millihertz(), 1_000_000);
        assert_eq!(refresh.presentation_refresh_nsec(), 1_000_000);
    }
}
