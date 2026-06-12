#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba {
    red: u8,
    green: u8,
    blue: u8,
    alpha: u8,
}

impl Rgba {
    pub const TRANSPARENT: Self = Self::new(0, 0, 0, 0);

    pub const fn new(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    pub const fn rgb(red: u8, green: u8, blue: u8) -> Self {
        Self::new(red, green, blue, 255)
    }

    pub const fn red(self) -> u8 {
        self.red
    }

    pub const fn green(self) -> u8 {
        self.green
    }

    pub const fn blue(self) -> u8 {
        self.blue
    }

    pub const fn alpha(self) -> u8 {
        self.alpha
    }

    pub const fn with_alpha(self, alpha: u8) -> Self {
        Self { alpha, ..self }
    }

    pub const fn premultiplied_argb(self) -> u32 {
        let alpha = self.alpha as u32;
        let red = (self.red as u32 * alpha + 127) / 255;
        let green = (self.green as u32 * alpha + 127) / 255;
        let blue = (self.blue as u32 * alpha + 127) / 255;
        (alpha << 24) | (red << 16) | (green << 8) | blue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplied_argb_scales_color_channels_by_alpha() {
        assert_eq!(
            Rgba::new(100, 50, 10, 128).premultiplied_argb(),
            0x8032_1905
        );
    }
}
