//! Canonical finite ICCCM dimension arithmetic.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegalDimensionLattice {
    lower: u32,
    upper: u32,
    anchor: u32,
    step: u32,
}

impl LegalDimensionLattice {
    pub fn new(
        min: Option<u32>,
        max: Option<u32>,
        base: Option<u32>,
        increment: Option<u32>,
    ) -> Option<Self> {
        let lower = min.unwrap_or(1).max(base.unwrap_or(0));
        let upper = max.unwrap_or(u32::MAX);
        if lower > upper {
            return None;
        }

        Some(Self {
            lower,
            upper,
            anchor: base.unwrap_or(lower),
            step: increment.filter(|step| *step > 0).unwrap_or(1),
        })
        .filter(|lattice| lattice.anchor <= lattice.upper)
    }

    pub const fn lower(self) -> u32 {
        self.lower
    }

    pub const fn upper(self) -> u32 {
        self.upper
    }

    pub const fn anchor(self) -> u32 {
        self.anchor
    }

    pub const fn step(self) -> u32 {
        self.step
    }

    fn first(self) -> Option<u32> {
        self.align_up(self.lower)
    }

    pub fn align_up(self, value: u32) -> Option<u32> {
        let value = value.max(self.lower).max(self.anchor);
        let delta = u64::from(value.saturating_sub(self.anchor));
        let steps = delta.div_ceil(u64::from(self.step));
        let aligned =
            u64::from(self.anchor).saturating_add(steps.saturating_mul(u64::from(self.step)));
        (aligned <= u64::from(self.upper)).then_some(aligned as u32)
    }

    pub fn align_down(self, value: u32) -> Option<u32> {
        let value = value.min(self.upper);
        if value < self.anchor {
            return None;
        }
        let steps = u64::from(value - self.anchor) / u64::from(self.step);
        let aligned =
            u64::from(self.anchor).saturating_add(steps.saturating_mul(u64::from(self.step)));
        (aligned >= u64::from(self.lower)).then_some(aligned as u32)
    }

    pub const fn contains(self, value: u32) -> bool {
        value >= self.lower
            && value <= self.upper
            && value >= self.anchor
            && (value - self.anchor).is_multiple_of(self.step)
    }

    pub fn cardinality(self) -> u64 {
        self.first().map_or(0, |first| {
            u64::from(self.upper - first) / u64::from(self.step) + 1
        })
    }

    pub fn value_at(self, index: u64) -> Option<u32> {
        let value =
            u64::from(self.first()?).checked_add(index.checked_mul(u64::from(self.step))?)?;
        (value <= u64::from(self.upper)).then_some(value as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::LegalDimensionLattice;

    #[test]
    fn uses_lower_bound_as_the_anchor_without_a_base() {
        let lattice =
            LegalDimensionLattice::new(Some(5), Some(20), None, Some(4)).expect("valid lattice");
        assert_eq!(lattice.align_up(5), Some(5));
        assert_eq!(lattice.align_up(6), Some(9));
        assert_eq!(lattice.align_down(20), Some(17));
        assert!(lattice.contains(13));
        assert!(!lattice.contains(14));
    }

    #[test]
    fn base_and_increment_share_one_alignment_rule() {
        let lattice = LegalDimensionLattice::new(Some(33), Some(116), Some(8), Some(3))
            .expect("valid lattice");
        assert_eq!(lattice.align_up(33), Some(35));
        assert_eq!(lattice.align_down(84), Some(83));
        assert_eq!(lattice.value_at(15), Some(80));
        assert!(lattice.contains(80));
    }

    #[test]
    fn rejects_empty_ranges_and_reports_finite_cardinality() {
        assert!(LegalDimensionLattice::new(Some(20), Some(10), None, None).is_none());
        let lattice =
            LegalDimensionLattice::new(Some(8), Some(20), Some(8), Some(3)).expect("valid lattice");
        assert_eq!(lattice.cardinality(), 5);
        assert_eq!(lattice.value_at(5), None);
    }
}
