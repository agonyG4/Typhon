use super::exact_aspect::{ExactAspectRatio, largest_exact_aspect_pair};
use super::geometry::LayoutRect;
use super::lattice::LegalDimensionLattice;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LayoutConstraints {
    pub min_width: Option<u32>,
    pub min_height: Option<u32>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub base_width: Option<u32>,
    pub base_height: Option<u32>,
    pub width_increment: Option<u32>,
    pub height_increment: Option<u32>,
    pub min_aspect: Option<f64>,
    pub max_aspect: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintValidationError {
    WidthBounds,
    HeightBounds,
    ZeroWidthIncrement,
    ZeroHeightIncrement,
    InvalidMinAspect,
    InvalidMaxAspect,
    ReversedAspectRange,
}

impl LayoutConstraints {
    pub fn normalized(mut self) -> Self {
        if self
            .min_width
            .zip(self.max_width)
            .is_some_and(|(min, max)| min > max)
        {
            self.max_width = None;
        }
        if self
            .min_height
            .zip(self.max_height)
            .is_some_and(|(min, max)| min > max)
        {
            self.max_height = None;
        }
        if self.width_increment == Some(0) {
            self.width_increment = None;
        }
        if self.height_increment == Some(0) {
            self.height_increment = None;
        }
        if self
            .min_aspect
            .is_some_and(|aspect| !aspect.is_finite() || aspect <= 0.0)
        {
            self.min_aspect = None;
        }
        if self
            .max_aspect
            .is_some_and(|aspect| !aspect.is_finite() || aspect <= 0.0)
        {
            self.max_aspect = None;
        }
        if self
            .min_aspect
            .zip(self.max_aspect)
            .is_some_and(|(min, max)| min > max)
        {
            self.min_aspect = None;
            self.max_aspect = None;
        }
        self
    }

    pub fn validate(self) -> Result<Self, ConstraintValidationError> {
        if self
            .min_width
            .zip(self.max_width)
            .is_some_and(|(min, max)| min > max)
        {
            return Err(ConstraintValidationError::WidthBounds);
        }
        if self
            .min_height
            .zip(self.max_height)
            .is_some_and(|(min, max)| min > max)
        {
            return Err(ConstraintValidationError::HeightBounds);
        }
        if self.width_increment == Some(0) {
            return Err(ConstraintValidationError::ZeroWidthIncrement);
        }
        if self.height_increment == Some(0) {
            return Err(ConstraintValidationError::ZeroHeightIncrement);
        }
        if self
            .min_aspect
            .is_some_and(|aspect| !aspect.is_finite() || aspect <= 0.0)
        {
            return Err(ConstraintValidationError::InvalidMinAspect);
        }
        if self
            .max_aspect
            .is_some_and(|aspect| !aspect.is_finite() || aspect <= 0.0)
        {
            return Err(ConstraintValidationError::InvalidMaxAspect);
        }
        if self
            .min_aspect
            .zip(self.max_aspect)
            .is_some_and(|(min, max)| min > max)
        {
            return Err(ConstraintValidationError::ReversedAspectRange);
        }
        Ok(self)
    }

    /// Compatibility name for the independent lower-bound query.
    pub fn minimum_tile_size(self) -> Result<(u32, u32), ClientRectError> {
        self.independent_lower_bounds()
    }

    /// Return the first legal width and height independently.
    ///
    /// These values are cheap lower bounds only.  Aspect constraints couple
    /// the two dimensions and are deliberately resolved by exact feasibility
    /// queries instead of being folded into this scalar result.
    pub fn independent_lower_bounds(self) -> Result<(u32, u32), ClientRectError> {
        self.validate()
            .map_err(ClientRectError::InvalidConstraints)?;
        let width = minimum_dimension(
            self.min_width,
            self.max_width,
            self.base_width,
            self.width_increment,
        )
        .ok_or(ClientRectError::Infeasible)?;
        let height = minimum_dimension(
            self.min_height,
            self.max_height,
            self.base_height,
            self.height_increment,
        )
        .ok_or(ClientRectError::Infeasible)?;
        Ok((width, height))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientRectError {
    InvalidConstraints(ConstraintValidationError),
    Infeasible,
}

pub fn resolve_client_rect_within_tile(
    tile: LayoutRect,
    constraints: LayoutConstraints,
) -> Result<LayoutRect, ClientRectError> {
    resolve_client_rect_within_tile_with_stats(
        tile,
        constraints,
        &mut ClientResolutionStats::default(),
    )
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ClientResolutionStats {
    pub(crate) dimension_candidates: u64,
    pub(crate) interval_nodes: u64,
    pub(crate) exact_aspect_probes: u64,
}

pub(crate) fn resolve_client_rect_within_tile_with_stats(
    tile: LayoutRect,
    constraints: LayoutConstraints,
    stats: &mut ClientResolutionStats,
) -> Result<LayoutRect, ClientRectError> {
    constraints
        .validate()
        .map_err(ClientRectError::InvalidConstraints)?;
    let width_min = minimum_dimension(
        constraints.min_width,
        constraints.max_width,
        constraints.base_width,
        constraints.width_increment,
    )
    .ok_or(ClientRectError::Infeasible)?;
    let height_min = minimum_dimension(
        constraints.min_height,
        constraints.max_height,
        constraints.base_height,
        constraints.height_increment,
    )
    .ok_or(ClientRectError::Infeasible)?;
    let width_max = maximum_dimension(
        tile.width(),
        constraints.min_width,
        constraints.max_width,
        constraints.base_width,
        constraints.width_increment,
    )
    .ok_or(ClientRectError::Infeasible)?;
    let height_max = maximum_dimension(
        tile.height(),
        constraints.min_height,
        constraints.max_height,
        constraints.base_height,
        constraints.height_increment,
    )
    .ok_or(ClientRectError::Infeasible)?;

    let width_lattice = LegalDimensionLattice::new(
        Some(width_min),
        Some(width_max),
        constraints.base_width,
        constraints.width_increment,
    )
    .ok_or(ClientRectError::Infeasible)?;
    let height_lattice = LegalDimensionLattice::new(
        Some(height_min),
        Some(height_max),
        constraints.base_height,
        constraints.height_increment,
    )
    .ok_or(ClientRectError::Infeasible)?;
    let best = find_best_lattice_pair(width_lattice, height_lattice, constraints, stats);
    let (width, height) = best.ok_or(ClientRectError::Infeasible)?;
    let x_offset = (tile.width() - width) / 2;
    let y_offset = (tile.height() - height) / 2;
    LayoutRect::new(
        tile.x()
            .saturating_add(i32::try_from(x_offset).unwrap_or(i32::MAX)),
        tile.y()
            .saturating_add(i32::try_from(y_offset).unwrap_or(i32::MAX)),
        width,
        height,
    )
    .ok_or(ClientRectError::Infeasible)
}

const DIRECT_LATTICE_CANDIDATES: u64 = 100_000;

fn find_best_lattice_pair(
    width_lattice: LegalDimensionLattice,
    height_lattice: LegalDimensionLattice,
    constraints: LayoutConstraints,
    stats: &mut ClientResolutionStats,
) -> Option<(u32, u32)> {
    if let (Some(min), Some(max)) = (constraints.min_aspect, constraints.max_aspect)
        && min.to_bits() == max.to_bits()
        && let Some(ratio) = ExactAspectRatio::from_f64(min)
    {
        return largest_exact_aspect_pair(
            width_lattice,
            height_lattice,
            ratio,
            &mut stats.exact_aspect_probes,
        );
    }
    find_best_lattice_pair_with_stats(width_lattice, height_lattice, constraints, stats)
}

fn find_best_lattice_pair_with_stats(
    width_lattice: LegalDimensionLattice,
    height_lattice: LegalDimensionLattice,
    constraints: LayoutConstraints,
    stats: &mut ClientResolutionStats,
) -> Option<(u32, u32)> {
    if width_lattice.cardinality() <= height_lattice.cardinality() {
        search_widths(
            width_lattice,
            height_lattice,
            constraints,
            stats,
            0,
            width_lattice.cardinality().saturating_sub(1),
        )
    } else {
        search_heights(
            height_lattice,
            width_lattice,
            constraints,
            stats,
            0,
            height_lattice.cardinality().saturating_sub(1),
        )
    }
}

fn search_widths(
    widths: LegalDimensionLattice,
    heights: LegalDimensionLattice,
    constraints: LayoutConstraints,
    stats: &mut ClientResolutionStats,
    first: u64,
    last: u64,
) -> Option<(u32, u32)> {
    if first > last {
        return None;
    }
    stats.interval_nodes = stats.interval_nodes.saturating_add(1);
    let high = widths.value_at(last)?;
    if !continuous_width_interval_can_intersect(widths.value_at(first)?, high, heights, constraints)
    {
        return None;
    }
    if let Some(height) = largest_height_for_width(high, heights, constraints, stats) {
        return Some((high, height));
    }
    if first == last {
        return None;
    }
    if last.saturating_sub(first).saturating_add(1) <= DIRECT_LATTICE_CANDIDATES {
        for index in (first..=last).rev() {
            let width = widths.value_at(index)?;
            if let Some(height) = largest_height_for_width(width, heights, constraints, stats) {
                return Some((width, height));
            }
        }
        return None;
    }
    let middle = first + (last - first) / 2;
    search_widths(
        widths,
        heights,
        constraints,
        stats,
        middle.saturating_add(1),
        last,
    )
    .or_else(|| search_widths(widths, heights, constraints, stats, first, middle))
}

fn search_heights(
    heights: LegalDimensionLattice,
    widths: LegalDimensionLattice,
    constraints: LayoutConstraints,
    stats: &mut ClientResolutionStats,
    first: u64,
    last: u64,
) -> Option<(u32, u32)> {
    if first > last {
        return None;
    }
    stats.interval_nodes = stats.interval_nodes.saturating_add(1);
    let high = heights.value_at(last)?;
    if !continuous_height_interval_can_intersect(
        heights.value_at(first)?,
        high,
        widths,
        constraints,
    ) {
        return None;
    }
    if let Some(width) = largest_width_for_height(high, widths, constraints, stats) {
        return Some((width, high));
    }
    if first == last {
        return None;
    }
    if last.saturating_sub(first).saturating_add(1) <= DIRECT_LATTICE_CANDIDATES {
        for index in (first..=last).rev() {
            let height = heights.value_at(index)?;
            if let Some(width) = largest_width_for_height(height, widths, constraints, stats) {
                return Some((width, height));
            }
        }
        return None;
    }
    let middle = first + (last - first) / 2;
    search_heights(
        heights,
        widths,
        constraints,
        stats,
        middle.saturating_add(1),
        last,
    )
    .or_else(|| search_heights(heights, widths, constraints, stats, first, middle))
}

fn continuous_width_interval_can_intersect(
    low: u32,
    high: u32,
    other: LegalDimensionLattice,
    constraints: LayoutConstraints,
) -> bool {
    let low = f64::from(low);
    let high = f64::from(high);
    let other_low = f64::from(other.lower());
    let other_high = f64::from(other.upper());
    if let Some(max) = constraints.max_aspect
        && low / other_high > max
    {
        return false;
    }
    if let Some(min) = constraints.min_aspect
        && high / other_low < min
    {
        return false;
    }
    true
}

fn continuous_height_interval_can_intersect(
    low: u32,
    high: u32,
    widths: LegalDimensionLattice,
    constraints: LayoutConstraints,
) -> bool {
    let low = f64::from(low);
    let high = f64::from(high);
    let width_low = f64::from(widths.lower());
    let width_high = f64::from(widths.upper());
    if let Some(max) = constraints.max_aspect
        && width_low / high > max
    {
        return false;
    }
    if let Some(min) = constraints.min_aspect
        && width_high / low < min
    {
        return false;
    }
    true
}

fn largest_height_for_width(
    width: u32,
    heights: LegalDimensionLattice,
    constraints: LayoutConstraints,
    stats: &mut ClientResolutionStats,
) -> Option<u32> {
    stats.dimension_candidates = stats.dimension_candidates.saturating_add(1);
    let mut lower = heights.lower();
    let mut upper = heights.upper();
    if let Some(max) = constraints.max_aspect {
        lower = lower.max(ceil_aspect_ratio(width, max));
    }
    if let Some(min) = constraints.min_aspect {
        upper = upper.min(floor_aspect_ratio(width, min));
    }
    let mut candidate = heights.align_down(upper)?;
    loop {
        if candidate < lower {
            return None;
        }
        if aspect_is_legal(width, candidate, constraints) {
            return Some(candidate);
        }
        let aspect = f64::from(width) / f64::from(candidate);
        let next = if constraints.max_aspect.is_some_and(|max| aspect > max) {
            heights.align_up(candidate.saturating_add(1))?
        } else {
            heights.align_down(candidate.saturating_sub(1))?
        };
        if next < lower || next > upper {
            return None;
        }
        candidate = next;
    }
}

fn largest_width_for_height(
    height: u32,
    widths: LegalDimensionLattice,
    constraints: LayoutConstraints,
    stats: &mut ClientResolutionStats,
) -> Option<u32> {
    stats.dimension_candidates = stats.dimension_candidates.saturating_add(1);
    let mut lower = widths.lower();
    let mut upper = widths.upper();
    if let Some(min) = constraints.min_aspect {
        lower = lower.max(ceil_aspect_product(height, min));
    }
    if let Some(max) = constraints.max_aspect {
        upper = upper.min(floor_aspect_product(height, max));
    }
    let mut candidate = widths.align_down(upper)?;
    loop {
        if candidate < lower {
            return None;
        }
        if aspect_is_legal(candidate, height, constraints) {
            return Some(candidate);
        }
        let aspect = f64::from(candidate) / f64::from(height);
        let next = if constraints.min_aspect.is_some_and(|min| aspect < min) {
            widths.align_up(candidate.saturating_add(1))?
        } else {
            widths.align_down(candidate.saturating_sub(1))?
        };
        if next < lower || next > upper {
            return None;
        }
        candidate = next;
    }
}

fn minimum_dimension(
    min: Option<u32>,
    max: Option<u32>,
    base: Option<u32>,
    increment: Option<u32>,
) -> Option<u32> {
    let lattice = LegalDimensionLattice::new(min, max, base, increment)?;
    lattice.align_up(lattice.lower())
}

fn maximum_dimension(
    limit: u32,
    min: Option<u32>,
    max: Option<u32>,
    base: Option<u32>,
    increment: Option<u32>,
) -> Option<u32> {
    LegalDimensionLattice::new(min, max, base, increment)?.align_down(limit)
}

fn aspect_is_legal(width: u32, height: u32, constraints: LayoutConstraints) -> bool {
    let aspect = f64::from(width) / f64::from(height);
    constraints.min_aspect.is_none_or(|min| aspect >= min)
        && constraints.max_aspect.is_none_or(|max| aspect <= max)
}

fn ceil_aspect_ratio(value: u32, aspect: f64) -> u32 {
    (f64::from(value) / aspect)
        .ceil()
        .clamp(1.0, f64::from(u32::MAX)) as u32
}

fn floor_aspect_ratio(value: u32, aspect: f64) -> u32 {
    (f64::from(value) / aspect)
        .floor()
        .clamp(1.0, f64::from(u32::MAX)) as u32
}

fn ceil_aspect_product(value: u32, aspect: f64) -> u32 {
    (f64::from(value) * aspect)
        .ceil()
        .clamp(1.0, f64::from(u32::MAX)) as u32
}

fn floor_aspect_product(value: u32, aspect: f64) -> u32 {
    (f64::from(value) * aspect)
        .floor()
        .clamp(1.0, f64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::super::geometry::LayoutRect;
    use super::super::lattice::LegalDimensionLattice;
    use super::{
        ClientRectError, LayoutConstraints, resolve_client_rect_within_tile,
        resolve_client_rect_within_tile_with_stats,
    };

    fn tile(width: u32, height: u32) -> LayoutRect {
        LayoutRect::new(100, 50, width, height).expect("valid test tile")
    }

    #[test]
    fn rejects_invalid_icccm_constraint_values() {
        assert!(
            LayoutConstraints {
                min_width: Some(900),
                max_width: Some(800),
                ..LayoutConstraints::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            LayoutConstraints {
                width_increment: Some(0),
                ..LayoutConstraints::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            LayoutConstraints {
                min_aspect: Some(f64::NAN),
                ..LayoutConstraints::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            LayoutConstraints {
                min_aspect: Some(2.0),
                max_aspect: Some(1.0),
                ..LayoutConstraints::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn resolves_fixed_size_inside_a_larger_tile() {
        let constraints = LayoutConstraints {
            min_width: Some(640),
            max_width: Some(640),
            min_height: Some(480),
            max_height: Some(480),
            ..LayoutConstraints::default()
        };
        let resolved = resolve_client_rect_within_tile(tile(1000, 800), constraints)
            .expect("fixed client fits");
        assert_eq!(resolved.width(), 640);
        assert_eq!(resolved.height(), 480);
        assert_eq!(resolved.x(), 280);
        assert_eq!(resolved.y(), 210);
    }

    #[test]
    fn chooses_largest_increment_aligned_size() {
        let constraints = LayoutConstraints {
            base_width: Some(320),
            width_increment: Some(8),
            base_height: Some(200),
            height_increment: Some(10),
            ..LayoutConstraints::default()
        };
        let resolved = resolve_client_rect_within_tile(tile(997, 607), constraints)
            .expect("increment-aligned client fits");
        assert_eq!(resolved.width(), 992);
        assert_eq!(resolved.height(), 600);
    }

    #[test]
    fn resolves_aspect_and_increments_without_unbounded_search() {
        let constraints = LayoutConstraints {
            min_width: Some(320),
            min_height: Some(200),
            base_width: Some(320),
            base_height: Some(200),
            width_increment: Some(8),
            height_increment: Some(10),
            min_aspect: Some(1.5),
            max_aspect: Some(1.8),
            ..LayoutConstraints::default()
        };
        let resolved = resolve_client_rect_within_tile(tile(997, 607), constraints)
            .expect("aspect and increment constraints are jointly feasible");
        assert!(resolved.width() <= 997);
        assert!(resolved.height() <= 607);
        let aspect = f64::from(resolved.width()) / f64::from(resolved.height());
        assert!((1.5..=1.8).contains(&aspect));
        assert_eq!((resolved.width() - 320) % 8, 0);
        assert_eq!((resolved.height() - 200) % 10, 0);
    }

    #[test]
    fn reports_an_impossible_aspect_and_fixed_size_combination() {
        let constraints = LayoutConstraints {
            min_width: Some(640),
            max_width: Some(640),
            min_height: Some(480),
            max_height: Some(480),
            min_aspect: Some(2.0),
            ..LayoutConstraints::default()
        };
        assert!(matches!(
            resolve_client_rect_within_tile(tile(1000, 800), constraints),
            Err(ClientRectError::Infeasible)
        ));
    }

    #[test]
    fn resolves_the_confirmed_80_by_45_icccm_regression() {
        let constraints = LayoutConstraints {
            min_width: Some(33),
            max_width: Some(116),
            base_width: Some(8),
            width_increment: Some(3),
            min_height: Some(8),
            max_height: Some(106),
            base_height: Some(5),
            min_aspect: Some(16.0 / 9.0),
            max_aspect: Some(16.0 / 9.0),
            ..LayoutConstraints::default()
        };
        let resolved = resolve_client_rect_within_tile(tile(84, 67), constraints)
            .expect("the finite legal 80x45 size must be found");
        assert_eq!((resolved.width(), resolved.height()), (80, 45));
    }

    #[test]
    fn huge_exact_aspect_disjoint_lattices_are_bounded() {
        let constraints = LayoutConstraints {
            min_width: Some(1),
            max_width: Some(u32::MAX - 294),
            base_width: Some(1),
            width_increment: Some(2),
            min_height: Some(2),
            max_height: Some(u32::MAX - 295),
            base_height: Some(2),
            height_increment: Some(2),
            min_aspect: Some(1.0),
            max_aspect: Some(1.0),
        };
        let tile = tile(u32::MAX - 294, u32::MAX - 295);
        let mut stats = super::ClientResolutionStats::default();
        assert_eq!(
            resolve_client_rect_within_tile_with_stats(tile, constraints, &mut stats),
            Err(ClientRectError::Infeasible)
        );
        assert!(stats.exact_aspect_probes <= 8, "{stats:?}");
        assert!(stats.dimension_candidates <= 1_000, "{stats:?}");
        assert!(stats.interval_nodes <= 1_000, "{stats:?}");
    }

    #[test]
    fn huge_exact_aspect_lattice_selects_the_largest_solution_without_enumeration() {
        let maximum = u32::MAX - 294;
        let constraints = LayoutConstraints {
            min_width: Some(1),
            max_width: Some(maximum),
            base_width: Some(1),
            width_increment: Some(2),
            min_height: Some(1),
            max_height: Some(maximum),
            base_height: Some(1),
            height_increment: Some(2),
            min_aspect: Some(1.0),
            max_aspect: Some(1.0),
        };
        let mut stats = super::ClientResolutionStats::default();
        let resolved = resolve_client_rect_within_tile_with_stats(
            tile(maximum, maximum),
            constraints,
            &mut stats,
        )
        .expect("the largest exact-aspect lattice point must fit");
        assert_eq!((resolved.width(), resolved.height()), (maximum, maximum));
        assert!(stats.exact_aspect_probes <= 8, "{stats:?}");
        assert!(stats.dimension_candidates <= 1_000, "{stats:?}");
        assert!(stats.interval_nodes <= 1_000, "{stats:?}");
    }

    #[test]
    fn independent_lower_bounds_do_not_expand_for_exact_aspect() {
        let constraints = LayoutConstraints {
            min_aspect: Some(1.0),
            max_aspect: Some(1.0),
            ..LayoutConstraints::default()
        };
        assert_eq!(constraints.independent_lower_bounds().unwrap(), (1, 1));
    }

    #[test]
    fn independent_lower_bounds_align_each_axis_without_proving_aspect_feasibility() {
        let constraints = LayoutConstraints {
            min_width: Some(5),
            base_width: Some(2),
            width_increment: Some(4),
            min_height: Some(6),
            base_height: Some(3),
            height_increment: Some(4),
            min_aspect: Some(1.0),
            max_aspect: Some(1.0),
            ..LayoutConstraints::default()
        };
        assert_eq!(constraints.independent_lower_bounds().unwrap(), (6, 7));
        assert_eq!(
            resolve_client_rect_within_tile(tile(100, 100), constraints),
            Err(ClientRectError::Infeasible)
        );
    }

    #[test]
    fn exhaustive_small_lattice_oracle_matches_production_maximum() {
        let cases = [
            LayoutConstraints::default(),
            LayoutConstraints {
                min_width: Some(3),
                min_height: Some(2),
                ..LayoutConstraints::default()
            },
            LayoutConstraints {
                base_width: Some(4),
                width_increment: Some(3),
                ..LayoutConstraints::default()
            },
            LayoutConstraints {
                base_height: Some(3),
                height_increment: Some(2),
                ..LayoutConstraints::default()
            },
            LayoutConstraints {
                base_width: Some(2),
                width_increment: Some(3),
                base_height: Some(3),
                height_increment: Some(2),
                ..LayoutConstraints::default()
            },
            LayoutConstraints {
                min_width: Some(4),
                max_width: Some(9),
                min_height: Some(3),
                max_height: Some(8),
                ..LayoutConstraints::default()
            },
            LayoutConstraints {
                min_aspect: Some(1.5),
                ..LayoutConstraints::default()
            },
            LayoutConstraints {
                max_aspect: Some(1.5),
                ..LayoutConstraints::default()
            },
            LayoutConstraints {
                min_aspect: Some(1.5),
                max_aspect: Some(1.5),
                ..LayoutConstraints::default()
            },
            LayoutConstraints {
                min_width: Some(3),
                base_width: Some(2),
                width_increment: Some(3),
                min_height: Some(2),
                base_height: Some(3),
                height_increment: Some(2),
                min_aspect: Some(4.0 / 3.0),
                max_aspect: Some(2.0),
                ..LayoutConstraints::default()
            },
        ];

        for width in 4..=18 {
            for height in 4..=18 {
                for constraints in cases {
                    let expected = exhaustive_oracle(width, height, constraints);
                    let actual = resolve_client_rect_within_tile(tile(width, height), constraints)
                        .ok()
                        .map(|rect| (rect.width(), rect.height()));
                    assert_eq!(
                        actual, expected,
                        "tile={width}x{height}, constraints={constraints:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn large_aspect_ranges_use_bounded_interval_work() {
        let constraints = LayoutConstraints {
            base_width: Some(8),
            width_increment: Some(3),
            base_height: Some(5),
            min_aspect: Some(16.0 / 9.0),
            max_aspect: Some(16.0 / 9.0),
            ..LayoutConstraints::default()
        };
        let mut stats = super::ClientResolutionStats::default();
        let resolved = super::resolve_client_rect_within_tile_with_stats(
            LayoutRect::new(0, 0, 4_000_000_000, 4_000_000_000).expect("large tile"),
            constraints,
            &mut stats,
        )
        .expect("large finite range is feasible");
        assert!(resolved.width() > 1_000_000_000);
        assert!(stats.dimension_candidates < 1_000);
    }

    fn exhaustive_oracle(
        tile_width: u32,
        tile_height: u32,
        constraints: LayoutConstraints,
    ) -> Option<(u32, u32)> {
        let widths = LegalDimensionLattice::new(
            constraints.min_width,
            constraints.max_width.map(|max| max.min(tile_width)),
            constraints.base_width,
            constraints.width_increment,
        )?;
        let heights = LegalDimensionLattice::new(
            constraints.min_height,
            constraints.max_height.map(|max| max.min(tile_height)),
            constraints.base_height,
            constraints.height_increment,
        )?;
        let mut best: Option<(u32, u32)> = None;
        for width_index in 0..widths.cardinality() {
            let width = widths.value_at(width_index)?;
            if width > tile_width {
                break;
            }
            for height_index in 0..heights.cardinality() {
                let height = heights.value_at(height_index)?;
                if height > tile_height {
                    break;
                }
                let aspect = f64::from(width) / f64::from(height);
                if constraints.min_aspect.is_some_and(|min| aspect < min)
                    || constraints.max_aspect.is_some_and(|max| aspect > max)
                {
                    continue;
                }
                let candidate = (width, height);
                let area = u64::from(width) * u64::from(height);
                if best.is_none_or(|current| {
                    area > u64::from(current.0) * u64::from(current.1)
                        || (area == u64::from(current.0) * u64::from(current.1)
                            && candidate > current)
                }) {
                    best = Some(candidate);
                }
            }
        }
        best
    }
}
