/// A finite, premultiplied presentation opacity in the visible interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationOpacity(f64);

impl PresentationOpacity {
    /// The fully transparent presentation value.
    pub const TRANSPARENT: Self = Self(0.0);
    /// The identity presentation value.
    pub const OPAQUE: Self = Self(1.0);

    /// Creates an opacity when `value` is finite and lies in `[0, 1]`.
    pub fn new(value: f64) -> Option<Self> {
        value
            .is_finite()
            .then_some(value)
            .filter(|value| (0.0..=1.0).contains(value))
            .map(Self)
    }

    /// Returns the scalar opacity.
    pub const fn get(self) -> f64 {
        self.0
    }

    /// Returns whether this value is fully opaque.
    pub const fn is_opaque(self) -> bool {
        self.0 == 1.0
    }

    /// Returns whether this value is fully transparent.
    pub const fn is_transparent(self) -> bool {
        self.0 == 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::PresentationOpacity;

    #[test]
    fn presentation_opacity_rejects_non_finite_values() {
        assert!(PresentationOpacity::new(f64::NAN).is_none());
        assert!(PresentationOpacity::new(f64::INFINITY).is_none());
        assert!(PresentationOpacity::new(f64::NEG_INFINITY).is_none());
    }

    #[test]
    fn presentation_opacity_rejects_out_of_range_values() {
        assert!(PresentationOpacity::new(-f64::EPSILON).is_none());
        assert!(PresentationOpacity::new(1.0 + f64::EPSILON).is_none());
    }

    #[test]
    fn presentation_opacity_accepts_boundaries_and_helpers() {
        let transparent = PresentationOpacity::new(0.0).expect("transparent is valid");
        let opaque = PresentationOpacity::new(1.0).expect("opaque is valid");

        assert_eq!(transparent, PresentationOpacity::TRANSPARENT);
        assert_eq!(opaque, PresentationOpacity::OPAQUE);
        assert_eq!(transparent.get(), 0.0);
        assert_eq!(opaque.get(), 1.0);
        assert!(transparent.is_transparent());
        assert!(!transparent.is_opaque());
        assert!(opaque.is_opaque());
        assert!(!opaque.is_transparent());
    }
}
