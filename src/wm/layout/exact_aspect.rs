use super::lattice::LegalDimensionLattice;

/// A reduced positive rational aspect ratio used by the exact lattice path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExactAspectRatio {
    numerator: u32,
    denominator: u32,
}

impl ExactAspectRatio {
    pub(crate) fn from_f64(value: f64) -> Option<Self> {
        if !value.is_finite() || value <= 0.0 {
            return None;
        }

        let mut remainder = value;
        let mut previous_numerator = 0u128;
        let mut numerator = 1u128;
        let mut previous_denominator = 1u128;
        let mut denominator = 0u128;
        for _ in 0..64 {
            let integer = remainder.floor();
            if integer > f64::from(u32::MAX) {
                return None;
            }
            let integer = integer as u128;
            let next_numerator = integer
                .checked_mul(numerator)?
                .checked_add(previous_numerator)?;
            let next_denominator = integer
                .checked_mul(denominator)?
                .checked_add(previous_denominator)?;
            if next_numerator > u128::from(u32::MAX) || next_denominator > u128::from(u32::MAX) {
                break;
            }
            previous_numerator = numerator;
            numerator = next_numerator;
            previous_denominator = denominator;
            denominator = next_denominator;

            if (value - numerator as f64 / denominator as f64).abs() <= 1e-14 * value.abs().max(1.0)
            {
                return Some(Self {
                    numerator: u32::try_from(numerator).ok()?,
                    denominator: u32::try_from(denominator).ok()?,
                });
            }

            let fractional = remainder - integer as f64;
            if fractional.abs() <= f64::EPSILON {
                break;
            }
            remainder = 1.0 / fractional;
        }

        None
    }

    pub(crate) const fn numerator(self) -> u32 {
        self.numerator
    }

    pub(crate) const fn denominator(self) -> u32 {
        self.denominator
    }
}

/// Solve `width / height == ratio` over two finite legal dimension lattices.
///
/// Both dimensions are represented as `k * p` and `k * q`.  The lattice
/// congruences become two linear congruences in `k`, which are combined with
/// a generalized CRT.  The returned `k` is selected arithmetically, so this
/// path never enumerates the legal dimensions.
pub(crate) fn largest_exact_aspect_pair(
    widths: LegalDimensionLattice,
    heights: LegalDimensionLattice,
    ratio: ExactAspectRatio,
    probes: &mut u64,
) -> Option<(u32, u32)> {
    *probes = probes.saturating_add(1);
    let p = u64::from(ratio.numerator());
    let q = u64::from(ratio.denominator());
    let lower = ceil_div(u64::from(widths.lower()), p)
        .max(ceil_div(u64::from(heights.lower()), q))
        .max(1);
    let upper = (u64::from(widths.upper()) / p).min(u64::from(heights.upper()) / q);
    if lower > upper {
        return None;
    }

    let width_congruence = linear_congruence(
        p,
        u64::from(widths.anchor()) % u64::from(widths.step()),
        u64::from(widths.step()),
    )?;
    *probes = probes.saturating_add(1);
    let height_congruence = linear_congruence(
        q,
        u64::from(heights.anchor()) % u64::from(heights.step()),
        u64::from(heights.step()),
    )?;
    *probes = probes.saturating_add(1);
    let (residue, modulus) = combine_congruences(width_congruence, height_congruence)?;
    *probes = probes.saturating_add(1);
    let k = largest_congruent_at_or_below(lower, upper, residue, modulus)?;
    let width = u32::try_from(u128::from(k) * u128::from(p)).ok()?;
    let height = u32::try_from(u128::from(k) * u128::from(q)).ok()?;
    Some((width, height))
}

fn ceil_div(value: u64, divisor: u64) -> u64 {
    value / divisor + u64::from(!value.is_multiple_of(divisor))
}

fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn linear_congruence(coefficient: u64, residue: u64, modulus: u64) -> Option<(u64, u64)> {
    if modulus == 1 {
        return Some((0, 1));
    }
    let divisor = gcd(coefficient, modulus);
    if !residue.is_multiple_of(divisor) {
        return None;
    }
    let coefficient = coefficient / divisor;
    let residue = residue / divisor;
    let modulus = modulus / divisor;
    let inverse = mod_inverse(coefficient % modulus, modulus)?;
    Some((
        (u128::from(residue) * u128::from(inverse) % u128::from(modulus)) as u64,
        modulus,
    ))
}

fn combine_congruences(left: (u64, u64), right: (u64, u64)) -> Option<(u64, u64)> {
    let (left_residue, left_modulus) = left;
    let (right_residue, right_modulus) = right;
    if left_modulus == 1 {
        return Some((right_residue % right_modulus, right_modulus));
    }
    if right_modulus == 1 {
        return Some((left_residue % left_modulus, left_modulus));
    }
    let divisor = gcd(left_modulus, right_modulus);
    let difference = i128::from(right_residue) - i128::from(left_residue);
    if difference % i128::from(divisor) != 0 {
        return None;
    }
    let reduced_modulus = right_modulus / divisor;
    let step = if reduced_modulus == 1 {
        0
    } else {
        let inverse = mod_inverse((left_modulus / divisor) % reduced_modulus, reduced_modulus)?;
        let difference = difference / i128::from(divisor);
        let difference = difference.rem_euclid(i128::from(reduced_modulus)) as u64;
        (u128::from(difference) * u128::from(inverse) % u128::from(reduced_modulus)) as u64
    };
    let modulus = u128::from(left_modulus / divisor) * u128::from(right_modulus);
    let residue =
        (u128::from(left_residue) + u128::from(left_modulus) * u128::from(step)) % modulus;
    Some((u64::try_from(residue).ok()?, u64::try_from(modulus).ok()?))
}

fn largest_congruent_at_or_below(
    lower: u64,
    upper: u64,
    residue: u64,
    modulus: u64,
) -> Option<u64> {
    if lower > upper {
        return None;
    }
    let residue = residue % modulus;
    if upper < residue {
        return None;
    }
    let candidate =
        u128::from(residue) + u128::from((upper - residue) / modulus) * u128::from(modulus);
    (candidate >= u128::from(lower)).then_some(candidate as u64)
}

fn mod_inverse(value: u64, modulus: u64) -> Option<u64> {
    if modulus == 1 {
        return Some(0);
    }
    let mut old_r = i128::from(value);
    let mut r = i128::from(modulus);
    let mut old_s = 1i128;
    let mut s = 0i128;
    while r != 0 {
        let quotient = old_r / r;
        (old_r, r) = (r, old_r - quotient * r);
        (old_s, s) = (s, old_s - quotient * s);
    }
    (old_r == 1).then_some(old_s.rem_euclid(i128::from(modulus)) as u64)
}

#[cfg(test)]
mod tests {
    use super::{ExactAspectRatio, largest_exact_aspect_pair};
    use crate::wm::layout::lattice::LegalDimensionLattice;

    #[test]
    fn recovers_common_protocol_rational_aspects() {
        assert_eq!(
            ExactAspectRatio::from_f64(1.0),
            Some(ExactAspectRatio {
                numerator: 1,
                denominator: 1
            })
        );
        assert_eq!(
            ExactAspectRatio::from_f64(16.0 / 9.0),
            Some(ExactAspectRatio {
                numerator: 16,
                denominator: 9
            })
        );
    }

    #[test]
    fn proves_parity_disjointness_without_dimension_enumeration() {
        let widths = LegalDimensionLattice::new(Some(1), Some(u32::MAX), Some(1), Some(2)).unwrap();
        let heights =
            LegalDimensionLattice::new(Some(2), Some(u32::MAX - 1), Some(2), Some(2)).unwrap();
        let mut probes = 0;
        assert_eq!(
            largest_exact_aspect_pair(
                widths,
                heights,
                ExactAspectRatio {
                    numerator: 1,
                    denominator: 1
                },
                &mut probes,
            ),
            None
        );
        assert!(probes <= 4);
    }

    #[test]
    fn combines_compatible_non_coprime_congruences() {
        let widths = LegalDimensionLattice::new(Some(1), Some(20), Some(1), Some(4)).unwrap();
        let heights = LegalDimensionLattice::new(Some(3), Some(20), Some(3), Some(6)).unwrap();
        let mut probes = 0;
        assert_eq!(
            largest_exact_aspect_pair(
                widths,
                heights,
                ExactAspectRatio {
                    numerator: 1,
                    denominator: 1
                },
                &mut probes,
            ),
            Some((9, 9))
        );
    }

    #[test]
    fn rejects_incompatible_non_coprime_congruences() {
        let widths = LegalDimensionLattice::new(Some(1), Some(100), Some(1), Some(4)).unwrap();
        let heights = LegalDimensionLattice::new(Some(2), Some(100), Some(2), Some(6)).unwrap();
        let mut probes = 0;
        assert_eq!(
            largest_exact_aspect_pair(
                widths,
                heights,
                ExactAspectRatio {
                    numerator: 1,
                    denominator: 1
                },
                &mut probes,
            ),
            None
        );
    }
}
