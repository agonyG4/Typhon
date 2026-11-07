use super::{EffectFootprint, MAX_EFFECT_REGION_RECTS};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffectRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl EffectRect {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        let right = i64::from(x) + i64::from(width);
        let bottom = i64::from(y) + i64::from(height);
        if right > i64::from(i32::MAX) || bottom > i64::from(i32::MAX) {
            return None;
        }
        Some(Self {
            x,
            y,
            width,
            height,
        })
    }

    pub fn right(self) -> i32 {
        (i64::from(self.x) + i64::from(self.width)) as i32
    }

    pub fn bottom(self) -> i32 {
        (i64::from(self.y) + i64::from(self.height)) as i32
    }

    pub fn intersect(self, other: Self) -> Option<Self> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > left && bottom > top).then(|| {
            Self::new(
                left,
                top,
                (i64::from(right) - i64::from(left)) as u32,
                (i64::from(bottom) - i64::from(top)) as u32,
            )
            .expect("intersection of valid effect rectangles must be valid")
        })
    }

    pub(crate) fn union(self, other: Self) -> Option<Self> {
        let left = self.x.min(other.x);
        let top = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        let width = i64::from(right) - i64::from(left);
        let height = i64::from(bottom) - i64::from(top);
        (width > 0 && height > 0 && width <= i64::from(u32::MAX) && height <= i64::from(u32::MAX))
            .then(|| Self::new(left, top, width as u32, height as u32))
            .flatten()
    }

    fn expanded(self, radius_x: u32, radius_y: u32, bounds: Self) -> Option<Self> {
        let left = (i64::from(self.x) - i64::from(radius_x)).max(i64::from(bounds.x));
        let top = (i64::from(self.y) - i64::from(radius_y)).max(i64::from(bounds.y));
        let right = (i64::from(self.right()) + i64::from(radius_x)).min(i64::from(bounds.right()));
        let bottom =
            (i64::from(self.bottom()) + i64::from(radius_y)).min(i64::from(bounds.bottom()));
        (right > left && bottom > top).then(|| {
            Self::new(
                left as i32,
                top as i32,
                (right - left) as u32,
                (bottom - top) as u32,
            )
            .expect("clipped expansion of valid effect rectangles must be valid")
        })
    }

    /// Whether `other` lies entirely inside this rect.
    pub fn contains_rect(self, other: Self) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }

    pub fn contains_point(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EffectRegion {
    rects: Vec<EffectRect>,
    conservative_full: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedRegionIntersection {
    pub region: EffectRegion,
    pub overflowed: bool,
    pub input_rect_count: usize,
    pub clip_rect_count: usize,
    pub duplicate_rects_removed: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedRegionDisjointification {
    pub region: EffectRegion,
    pub overflowed: bool,
    pub duplicate_rects_removed: usize,
    pub overlap_fragments_generated: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EffectRegionClipFallback {
    pub(crate) input_rect_count: usize,
    pub(crate) clip_rect_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectDamageSnapshot {
    pub identity: u64,
    pub region: EffectRegion,
}

impl EffectDamageSnapshot {
    pub const fn new(identity: u64, region: EffectRegion) -> Self {
        Self { identity, region }
    }
}

pub trait EffectTransitionSource {
    fn effect_identity(&self) -> Option<u64>;
    fn effect_region(&self) -> &EffectRegion;
}

impl EffectTransitionSource for EffectRegion {
    fn effect_identity(&self) -> Option<u64> {
        None
    }

    fn effect_region(&self) -> &EffectRegion {
        self
    }
}

impl EffectTransitionSource for EffectDamageSnapshot {
    fn effect_identity(&self) -> Option<u64> {
        Some(self.identity)
    }

    fn effect_region(&self) -> &EffectRegion {
        &self.region
    }
}

impl EffectRegion {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_rect(rect: EffectRect) -> Self {
        Self {
            rects: vec![rect],
            conservative_full: false,
        }
    }

    pub fn rects(&self) -> &[EffectRect] {
        &self.rects
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty() && !self.conservative_full
    }

    pub fn push(&mut self, rect: EffectRect) {
        if self.conservative_full {
            return;
        }
        if self.rects.contains(&rect) {
            return;
        }
        if self.rects.len() < MAX_EFFECT_REGION_RECTS {
            self.rects.push(rect);
            return;
        }
        let mut bounds = rect;
        for existing in &self.rects {
            match bounds.union(*existing) {
                Some(union) => bounds = union,
                None => {
                    self.rects.clear();
                    self.conservative_full = true;
                    return;
                }
            }
        }
        self.rects.clear();
        self.rects.push(bounds);
    }

    /// Like [`Self::push`], but treats the region as a coverage set: a rect
    /// already covered adds nothing, and rects the new one covers are
    /// dropped. Used where expanded sampling footprints produce many nested
    /// rects, so later region operations stay small.
    pub fn push_covering(&mut self, rect: EffectRect) {
        if self.conservative_full
            || self
                .rects
                .iter()
                .any(|existing| existing.contains_rect(rect))
        {
            return;
        }
        self.rects.retain(|existing| !rect.contains_rect(*existing));
        self.push(rect);
    }

    pub fn contains_point(&self, x: i32, y: i32) -> bool {
        self.conservative_full || self.rects.iter().any(|rect| rect.contains_point(x, y))
    }

    pub fn bounding_rect(&self) -> Option<EffectRect> {
        if self.conservative_full {
            return None;
        }
        let mut iter = self.rects.iter().copied();
        let first = iter.next()?;
        iter.try_fold(first, |left, right| left.union(right))
    }

    pub fn union(&self, other: &Self) -> Self {
        if self.conservative_full || other.conservative_full {
            return Self {
                rects: Vec::new(),
                conservative_full: true,
            };
        }
        let mut result = self.clone();
        for rect in &other.rects {
            result.push(*rect);
        }
        result
    }

    pub(crate) fn union_with_diagnostics(&self, other: &Self) -> (Self, bool) {
        if self.conservative_full || other.conservative_full {
            return (self.union(other), false);
        }
        let mut result = self.clone();
        let mut coalesced = false;
        for rect in &other.rects {
            if result.rects.len() >= MAX_EFFECT_REGION_RECTS && !result.rects.contains(rect) {
                coalesced = true;
            }
            result.push(*rect);
        }
        (result, coalesced)
    }

    pub fn intersect_rect(&self, bounds: EffectRect) -> Self {
        if self.conservative_full {
            return Self::from_rect(bounds);
        }
        let mut result = Self::empty();
        for rect in &self.rects {
            if let Some(intersection) = rect.intersect(bounds) {
                result.push(intersection);
            }
        }
        result
    }

    pub fn intersect(&self, other: &Self) -> Self {
        if self.conservative_full {
            return other.clone();
        }
        if other.conservative_full {
            return self.clone();
        }
        let mut result = Self::empty();
        for left in &self.rects {
            for right in &other.rects {
                if let Some(intersection) = left.intersect(*right) {
                    result.push(intersection);
                }
            }
        }
        result
    }

    /// Intersect this region while preserving the authoritative `clip`.
    ///
    /// The exact intersection is returned while it fits the bounded region
    /// representation. If it would overflow, the authoritative clip is a
    /// conservative result that cannot expose pixels outside the clip.
    pub fn intersect_bounded_within(&self, clip: &Self) -> Self {
        self.intersect_bounded_within_result(clip).region
    }

    pub fn intersect_bounded_within_result(&self, clip: &Self) -> BoundedRegionIntersection {
        let input_rect_count = self.rects.len();
        let clip_rect_count = clip.rects.len();
        if self.conservative_full {
            return BoundedRegionIntersection {
                region: clip.clone(),
                overflowed: false,
                input_rect_count,
                clip_rect_count,
                duplicate_rects_removed: 0,
            };
        }
        if clip.conservative_full {
            return BoundedRegionIntersection {
                region: self.clone(),
                overflowed: false,
                input_rect_count,
                clip_rect_count,
                duplicate_rects_removed: 0,
            };
        }
        let mut rects = Vec::new();
        let mut duplicate_rects_removed: usize = 0;
        for left in &self.rects {
            for right in &clip.rects {
                let Some(intersection) = left.intersect(*right) else {
                    continue;
                };
                if rects.contains(&intersection) {
                    duplicate_rects_removed += 1;
                    continue;
                }
                if rects.len() == MAX_EFFECT_REGION_RECTS {
                    return BoundedRegionIntersection {
                        region: clip.clone(),
                        overflowed: true,
                        input_rect_count,
                        clip_rect_count,
                        duplicate_rects_removed,
                    };
                }
                rects.push(intersection);
            }
        }
        BoundedRegionIntersection {
            region: Self {
                rects,
                conservative_full: false,
            },
            overflowed: false,
            input_rect_count,
            clip_rect_count,
            duplicate_rects_removed,
        }
    }

    /// Convert this bounded geometric region to single-coverage rectangles.
    ///
    /// The returned rectangles have exactly the same coverage while the
    /// decomposition fits the region bound. If it would not fit, callers must
    /// choose a conservative fallback appropriate to the region's semantics;
    /// the returned region is empty in that case so an overflow cannot be used
    /// accidentally as a partial result.
    pub fn disjoint_bounded(&self) -> BoundedRegionDisjointification {
        if self.conservative_full || self.rects.len() <= 1 {
            return BoundedRegionDisjointification {
                region: self.clone(),
                overflowed: false,
                duplicate_rects_removed: 0,
                overlap_fragments_generated: 0,
            };
        }

        let rects = &self.rects;
        let mut duplicate_rects_removed: usize = 0;
        let mut overlapping = false;
        for (index, rect) in rects.iter().enumerate() {
            for earlier in &rects[..index] {
                if earlier == rect {
                    duplicate_rects_removed += 1;
                    overlapping = true;
                } else if !overlapping && earlier.intersect(*rect).is_some() {
                    overlapping = true;
                }
            }
        }
        if !overlapping {
            return BoundedRegionDisjointification {
                region: self.clone(),
                overflowed: false,
                duplicate_rects_removed: 0,
                overlap_fragments_generated: 0,
            };
        }

        // Sweep horizontal bands between consecutive rect edges. Within a
        // band the covering x-intervals are merged, and a band whose intervals
        // match the band directly above extends those rects instead of adding
        // new ones. The result has exactly the input coverage, each pixel once.
        let mut edges = Vec::with_capacity(rects.len() * 2);
        for rect in rects {
            edges.push(rect.y);
            edges.push(rect.bottom());
        }
        edges.sort_unstable();
        edges.dedup();

        let mut disjoint: Vec<EffectRect> = Vec::with_capacity(rects.len());
        let mut intervals: Vec<(i32, i32)> = Vec::with_capacity(rects.len());
        let mut previous: Vec<(i32, i32)> = Vec::with_capacity(rects.len());
        let mut previous_start = 0;
        let mut previous_bottom = None;
        for band in edges.windows(2) {
            let (top, bottom) = (band[0], band[1]);
            intervals.clear();
            for rect in rects {
                if rect.y <= top && rect.bottom() >= bottom {
                    intervals.push((rect.x, rect.right()));
                }
            }
            if intervals.is_empty() {
                previous.clear();
                previous_bottom = None;
                continue;
            }
            intervals.sort_unstable();
            let mut merged = 0;
            for index in 1..intervals.len() {
                let (left, right) = intervals[index];
                if left <= intervals[merged].1 {
                    intervals[merged].1 = intervals[merged].1.max(right);
                } else {
                    merged += 1;
                    intervals[merged] = (left, right);
                }
            }
            intervals.truncate(merged + 1);
            let height = (bottom - top) as u32;
            if previous_bottom == Some(top) && previous == intervals {
                for rect in &mut disjoint[previous_start..] {
                    rect.height += height;
                }
            } else {
                if disjoint.len() + intervals.len() > MAX_EFFECT_REGION_RECTS {
                    return BoundedRegionDisjointification {
                        region: Self::empty(),
                        overflowed: true,
                        duplicate_rects_removed,
                        overlap_fragments_generated: disjoint.len() + intervals.len(),
                    };
                }
                previous_start = disjoint.len();
                for &(left, right) in &intervals {
                    disjoint.push(EffectRect {
                        x: left,
                        y: top,
                        width: (right - left) as u32,
                        height,
                    });
                }
                std::mem::swap(&mut previous, &mut intervals);
            }
            previous_bottom = Some(bottom);
        }

        let overlap_fragments_generated = disjoint.len();
        BoundedRegionDisjointification {
            region: Self {
                rects: disjoint,
                conservative_full: false,
            },
            overflowed: false,
            duplicate_rects_removed,
            overlap_fragments_generated,
        }
    }

    pub fn subtract(&self, excluded: &Self) -> Self {
        if self.is_empty() || excluded.conservative_full {
            return Self::empty();
        }
        if excluded.is_empty() || self.conservative_full {
            return self.clone();
        }
        let mut result = Self::empty();
        for source in &self.rects {
            let mut remaining = Self::from_rect(*source);
            for excluded_rect in &excluded.rects {
                if remaining.is_empty() {
                    break;
                }
                let mut next = Self::empty();
                for candidate in remaining.rects() {
                    for &piece in subtract_effect_rect(*candidate, *excluded_rect).as_slice() {
                        next.push(piece);
                    }
                }
                remaining = next;
            }
            for rect in remaining.rects() {
                result.push(*rect);
            }
        }
        result
    }

    pub fn intersects(&self, other: &Self) -> bool {
        if self.conservative_full || other.conservative_full {
            return !self.is_empty() && !other.is_empty();
        }
        self.rects.iter().any(|left| {
            other
                .rects
                .iter()
                .any(|right| left.intersect(*right).is_some())
        })
    }

    pub fn expand_clamped_xy(&self, radius_x: u32, radius_y: u32, bounds: EffectRect) -> Self {
        if self.conservative_full {
            return Self::from_rect(bounds);
        }
        let mut result = Self::empty();
        for rect in &self.rects {
            if let Some(expanded) = rect.expanded(radius_x, radius_y, bounds) {
                result.push(expanded);
            }
        }
        result
    }

    pub fn expand_clamped(&self, radius: u32, bounds: EffectRect) -> Self {
        self.expand_clamped_xy(radius, radius, bounds)
    }
}

/// At most four pieces remain after subtracting one rect from another.
struct SubtractedRects {
    rects: [EffectRect; 4],
    len: usize,
}

impl SubtractedRects {
    const fn new() -> Self {
        let empty = EffectRect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        Self {
            rects: [empty; 4],
            len: 0,
        }
    }

    fn push(&mut self, rect: EffectRect) {
        self.rects[self.len] = rect;
        self.len += 1;
    }

    fn as_slice(&self) -> &[EffectRect] {
        &self.rects[..self.len]
    }
}

fn subtract_effect_rect(source: EffectRect, excluded: EffectRect) -> SubtractedRects {
    let mut result = SubtractedRects::new();
    let Some(overlap) = source.intersect(excluded) else {
        result.push(source);
        return result;
    };
    let source_right = source.right();
    let source_bottom = source.bottom();
    let overlap_right = overlap.right();
    let overlap_bottom = overlap.bottom();
    if overlap.y > source.y {
        result.push(
            EffectRect::new(
                source.x,
                source.y,
                source.width,
                (overlap.y - source.y) as u32,
            )
            .unwrap(),
        );
    }
    if overlap_bottom < source_bottom {
        result.push(
            EffectRect::new(
                source.x,
                overlap_bottom,
                source.width,
                (source_bottom - overlap_bottom) as u32,
            )
            .unwrap(),
        );
    }
    if overlap.x > source.x {
        result.push(
            EffectRect::new(
                source.x,
                overlap.y,
                (overlap.x - source.x) as u32,
                overlap.height,
            )
            .unwrap(),
        );
    }
    if overlap_right < source_right {
        result.push(
            EffectRect::new(
                overlap_right,
                overlap.y,
                (source_right - overlap_right) as u32,
                overlap.height,
            )
            .unwrap(),
        );
    }
    result
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectDamagePlan {
    pub output_damage: EffectRegion,
    pub source_query_region: EffectRegion,
    pub dependency_region: EffectRegion,
    pub capture_region: EffectRegion,
    pub(crate) output_clip_fallback: Option<EffectRegionClipFallback>,
}

/// Returns the bounded final output influence of an effect instance.
///
/// Sample radii describe source dependency and remain part of the capture and
/// damage plan. Only output outsets extend the pixels contributed to output.
pub fn effect_output_influence_region(
    footprint: EffectFootprint,
    visible_region: &EffectRegion,
    output_bounds: EffectRect,
) -> EffectRegion {
    visible_region.expand_clamped_xy(
        footprint
            .output_outsets
            .left
            .max(footprint.output_outsets.right),
        footprint
            .output_outsets
            .top
            .max(footprint.output_outsets.bottom),
        output_bounds,
    )
}

/// Snap `rect` outward to the output-anchored `alignment` lattice, clamped to
/// the output. Used for multi-resolution capture domains so their texel grid
/// stays fixed on screen while the effect region resizes or moves by less than
/// one lattice cell. Pixels added this way lie beyond the effect's sampling
/// footprint, so they never influence visible output and stay out of the
/// damage and dependency plan.
pub fn align_domain_to_output_lattice(
    rect: EffectRect,
    alignment: u32,
    output_bounds: EffectRect,
) -> EffectRect {
    if alignment <= 1 {
        return rect;
    }
    let step = i64::from(alignment);
    let snap_down = |value: i64, origin: i64| origin + (value - origin).div_euclid(step) * step;
    let snap_up =
        |value: i64, origin: i64| origin + (value - origin + step - 1).div_euclid(step) * step;
    let (origin_x, origin_y) = (i64::from(output_bounds.x), i64::from(output_bounds.y));
    let left = snap_down(i64::from(rect.x), origin_x).max(origin_x);
    let top = snap_down(i64::from(rect.y), origin_y).max(origin_y);
    let right = snap_up(i64::from(rect.right()), origin_x)
        .min(i64::from(output_bounds.right()))
        .max(i64::from(rect.right()));
    let bottom = snap_up(i64::from(rect.bottom()), origin_y)
        .min(i64::from(output_bounds.bottom()))
        .max(i64::from(rect.bottom()));
    let left = left.min(i64::from(rect.x));
    let top = top.min(i64::from(rect.y));
    match (
        i32::try_from(left),
        i32::try_from(top),
        u32::try_from(right - left),
        u32::try_from(bottom - top),
    ) {
        (Ok(x), Ok(y), Ok(width), Ok(height)) => {
            EffectRect::new(x, y, width, height).unwrap_or(rect)
        }
        _ => rect,
    }
}

pub fn plan_effect_damage(
    footprint: EffectFootprint,
    visible_region: &EffectRegion,
    source_damage: &EffectRegion,
    output_bounds: EffectRect,
) -> EffectDamagePlan {
    let capture_region = visible_region.expand_clamped_xy(
        footprint.sample_radius_x.saturating_add(
            footprint
                .output_outsets
                .left
                .max(footprint.output_outsets.right),
        ),
        footprint.sample_radius_y.saturating_add(
            footprint
                .output_outsets
                .top
                .max(footprint.output_outsets.bottom),
        ),
        output_bounds,
    );
    let output_influence_region =
        effect_output_influence_region(footprint, visible_region, output_bounds);
    let output_damage_result = source_damage
        .expand_clamped_xy(
            footprint.sample_radius_x,
            footprint.sample_radius_y,
            output_bounds,
        )
        .intersect_bounded_within_result(&output_influence_region);
    let output_clip_fallback =
        output_damage_result
            .overflowed
            .then_some(EffectRegionClipFallback {
                input_rect_count: output_damage_result.input_rect_count,
                clip_rect_count: output_damage_result.clip_rect_count,
            });
    let output_damage = output_damage_result.region;
    EffectDamagePlan {
        output_damage,
        source_query_region: capture_region.clone(),
        dependency_region: output_influence_region,
        capture_region,
        output_clip_fallback,
    }
}

pub fn effect_transition_damage<O, N>(old: &O, new: &N) -> EffectRegion
where
    O: EffectTransitionSource,
    N: EffectTransitionSource,
{
    if old.effect_region() == new.effect_region() && old.effect_identity() == new.effect_identity()
    {
        EffectRegion::empty()
    } else if old.effect_region() == new.effect_region()
        && old.effect_identity().is_some()
        && old.effect_identity() != new.effect_identity()
    {
        old.effect_region().clone()
    } else if old.effect_identity().is_some()
        && old.effect_identity() == new.effect_identity()
        && old.effect_region() == new.effect_region()
    {
        EffectRegion::empty()
    } else {
        old.effect_region().union(new.effect_region())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::EffectOutsets;

    fn rounded_test_region(
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        radius: u32,
        segments: u32,
    ) -> EffectRegion {
        let mut region = EffectRegion::empty();
        for band in 0..segments {
            let top = radius * band / segments;
            let bottom = radius * (band + 1) / segments;
            let distance_from_center = radius - top;
            let remaining = radius * radius - distance_from_center * distance_from_center;
            let inset = radius - (f64::from(remaining).sqrt().floor() as u32);
            region.push(
                EffectRect::new(
                    x + inset as i32,
                    y + top as i32,
                    width - 2 * inset,
                    bottom - top,
                )
                .unwrap(),
            );
        }
        region.push(EffectRect::new(x, y + radius as i32, width, height - 2 * radius).unwrap());
        for band in (0..segments).rev() {
            let top = radius * band / segments;
            let bottom = radius * (band + 1) / segments;
            let distance_from_center = radius - top;
            let remaining = radius * radius - distance_from_center * distance_from_center;
            let inset = radius - (f64::from(remaining).sqrt().floor() as u32);
            region.push(
                EffectRect::new(
                    x + inset as i32,
                    y + height as i32 - bottom as i32,
                    width - 2 * inset,
                    bottom - top,
                )
                .unwrap(),
            );
        }
        assert_eq!(
            region.rects().len(),
            usize::try_from(segments * 2 + 1).unwrap()
        );
        region
    }

    #[test]
    fn local_tint_does_not_expand_damage() {
        let visible = EffectRegion::from_rect(EffectRect::new(0, 0, 200, 100).unwrap());
        let source = EffectRegion::from_rect(EffectRect::new(40, 20, 10, 10).unwrap());
        let plan = plan_effect_damage(
            EffectFootprint::ZERO,
            &visible,
            &source,
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
        );
        assert_eq!(plan.output_damage, source);
    }

    #[test]
    fn blur_expands_output_influence_from_source_damage() {
        let footprint = EffectFootprint::symmetric(12);
        let visible = EffectRegion::from_rect(EffectRect::new(0, 0, 200, 100).unwrap());
        let source = EffectRegion::from_rect(EffectRect::new(40, 20, 10, 10).unwrap());
        let plan = plan_effect_damage(
            footprint,
            &visible,
            &source,
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
        );
        assert!(plan.output_damage.contains_point(30, 10));
        assert!(plan.output_damage.contains_point(61, 41));
    }

    #[test]
    fn final_output_influence_uses_outsets_without_sample_radius() {
        let footprint = EffectFootprint {
            sample_radius_x: 50,
            sample_radius_y: 70,
            output_outsets: EffectOutsets {
                left: 10,
                right: 20,
                top: 30,
                bottom: 40,
            },
            grid_alignment: 1,
        };
        let visible = EffectRegion::from_rect(EffectRect::new(100, 100, 100, 100).unwrap());

        let influence = effect_output_influence_region(
            footprint,
            &visible,
            EffectRect::new(0, 0, 400, 300).unwrap(),
        );

        assert_eq!(
            influence.rects(),
            &[EffectRect::new(80, 60, 140, 180).unwrap()]
        );
    }

    #[test]
    fn dependency_region_tracks_lower_output_influence_not_blur_sample_padding() {
        let footprint = EffectFootprint::symmetric(8);
        let visible = EffectRegion::from_rect(EffectRect::new(100, 100, 100, 40).unwrap());
        let source = visible.clone();
        let plan = plan_effect_damage(
            footprint,
            &visible,
            &source,
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
        );
        assert!(plan.dependency_region.contains_point(100, 100));
        assert!(!plan.dependency_region.contains_point(92, 92));
    }

    #[test]
    fn aligned_domain_stays_on_the_output_lattice_while_the_region_grows() {
        let bounds = EffectRect::new(0, 0, 2560, 1440).unwrap();
        let footprint = EffectFootprint::symmetric(37).with_grid_alignment(16);
        let domains: Vec<_> = (300..=316)
            .map(|height| {
                let visible =
                    EffectRegion::from_rect(EffectRect::new(981, 361, 600, height).unwrap());
                let plan = plan_effect_damage(footprint, &visible, &EffectRegion::empty(), bounds);
                let capture = plan.capture_region.bounding_rect().unwrap();
                let domain =
                    align_domain_to_output_lattice(capture, footprint.grid_alignment, bounds);
                assert!(domain.contains_rect(capture));
                domain
            })
            .collect();
        for domain in &domains {
            assert_eq!(
                (
                    domain.x % 16,
                    domain.y % 16,
                    domain.width % 16,
                    domain.height % 16
                ),
                (0, 0, 0, 0),
                "{domain:?}"
            );
        }
        // Growth inside one lattice cell keeps the domain, and so the blur
        // grid, unchanged; the origin never moves.
        assert!(domains.windows(2).filter(|pair| pair[0] != pair[1]).count() <= 2);
        assert!(
            domains
                .iter()
                .all(|domain| (domain.x, domain.y) == (domains[0].x, domains[0].y))
        );
    }

    #[test]
    fn aligned_domain_clamps_to_output_edges() {
        let bounds = EffectRect::new(100, 50, 1000, 701).unwrap();
        let rect = EffectRect::new(105, 700, 30, 40).unwrap();
        assert_eq!(
            align_domain_to_output_lattice(rect, 16, bounds),
            EffectRect::new(100, 690, 48, 61).unwrap()
        );
        assert_eq!(align_domain_to_output_lattice(rect, 1, bounds), rect);
        let on_lattice = EffectRect::new(116, 66, 32, 16).unwrap();
        assert_eq!(
            align_domain_to_output_lattice(on_lattice, 16, bounds),
            on_lattice
        );
    }

    #[test]
    fn capture_region_is_clipped_to_output_bounds() {
        let footprint = EffectFootprint::symmetric(16);
        let visible = EffectRegion::from_rect(EffectRect::new(0, 0, 80, 40).unwrap());
        let plan = plan_effect_damage(
            footprint,
            &visible,
            &visible,
            EffectRect::new(0, 0, 100, 100).unwrap(),
        );
        assert!(!plan.capture_region.contains_point(-1, 0));
        assert!(plan.capture_region.contains_point(0, 0));
    }

    #[test]
    fn effect_removal_damages_previous_visible_region() {
        let old = EffectRegion::from_rect(EffectRect::new(10, 10, 50, 20).unwrap());
        let damage = effect_transition_damage(&old, &EffectRegion::empty());
        assert_eq!(damage, old);
    }

    #[test]
    fn unchanged_effect_identity_has_no_transition_damage() {
        let region = EffectRegion::from_rect(EffectRect::new(10, 10, 50, 20).unwrap());
        let old = EffectDamageSnapshot::new(7, region.clone());
        let new = EffectDamageSnapshot::new(7, region);
        assert!(effect_transition_damage(&old, &new).is_empty());
    }

    #[test]
    fn changed_effect_identity_damages_the_affected_region() {
        let region = EffectRegion::from_rect(EffectRect::new(10, 10, 50, 20).unwrap());
        let old = EffectDamageSnapshot::new(7, region.clone());
        let new = EffectDamageSnapshot::new(8, region.clone());
        assert_eq!(effect_transition_damage(&old, &new), region);
    }

    #[test]
    fn moving_effect_repairs_old_and_new_regions_without_widening_capture_domain() {
        let bounds = EffectRect::new(0, 0, 1920, 1080).unwrap();
        let footprint = EffectFootprint::symmetric(12);
        let old_region = EffectRegion::from_rect(EffectRect::new(120, 200, 240, 120).unwrap());
        let new_region = EffectRegion::from_rect(EffectRect::new(136, 202, 240, 120).unwrap());
        let old = EffectDamageSnapshot::new(7, old_region.clone());
        let new = EffectDamageSnapshot::new(7, new_region.clone());

        let repair = effect_transition_damage(&old, &new);
        assert!(repair.contains_point(120, 200));
        assert!(repair.contains_point(375, 321));
        assert!(!repair.contains_point(800, 800));

        let current = plan_effect_damage(footprint, &new_region, &EffectRegion::empty(), bounds);
        assert_eq!(
            current.capture_region,
            EffectRegion::from_rect(EffectRect::new(124, 190, 264, 144).unwrap())
        );
        assert!(!current.capture_region.contains_point(100, 200));
        assert!(current.output_damage.is_empty());
    }

    #[test]
    fn large_moving_effect_transition_composites_only_the_new_visible_area() {
        let bounds = EffectRect::new(0, 0, 1920, 1080).unwrap();
        let footprint = EffectFootprint::symmetric(16);
        let old_region = EffectRegion::from_rect(EffectRect::new(40, 40, 180, 80).unwrap());
        let new_region = EffectRegion::from_rect(EffectRect::new(1600, 900, 180, 80).unwrap());
        let old = EffectDamageSnapshot::new(11, old_region.clone());
        let new = EffectDamageSnapshot::new(11, new_region.clone());

        let repair = effect_transition_damage(&old, &new);
        assert!(repair.contains_point(40, 40));
        assert!(repair.contains_point(1779, 979));
        assert!(!repair.contains_point(900, 500));

        let current = plan_effect_damage(footprint, &new_region, &new_region, bounds);
        assert!(current.output_damage.contains_point(1600, 900));
        assert!(!current.output_damage.contains_point(40, 40));
        assert_eq!(
            current.capture_region,
            EffectRegion::from_rect(EffectRect::new(1584, 884, 212, 112).unwrap())
        );
    }

    #[test]
    fn moving_capture_domains_clip_independently_at_all_output_edges() {
        let bounds = EffectRect::new(0, 0, 100, 80).unwrap();
        let footprint = EffectFootprint::symmetric(8);
        for visible in [
            EffectRect::new(-6, 24, 32, 20).unwrap(),
            EffectRect::new(74, 24, 32, 20).unwrap(),
            EffectRect::new(34, -6, 32, 20).unwrap(),
            EffectRect::new(34, 66, 32, 20).unwrap(),
        ] {
            let plan = plan_effect_damage(
                footprint,
                &EffectRegion::from_rect(visible),
                &EffectRegion::empty(),
                bounds,
            );
            let capture = plan
                .capture_region
                .bounding_rect()
                .expect("clipped capture");
            assert!(capture.x >= bounds.x);
            assert!(capture.y >= bounds.y);
            assert!(capture.right() <= bounds.right());
            assert!(capture.bottom() <= bounds.bottom());
            assert!(capture.width < bounds.width || capture.height < bounds.height);
        }
    }

    #[test]
    fn overflow_uses_conservative_clamped_result() {
        let visible = EffectRegion::from_rect(EffectRect::new(i32::MAX - 4, 0, 4, 4).unwrap());
        let result = visible.expand_clamped(64, EffectRect::new(0, 0, 1920, 1080).unwrap());
        assert!(result.is_empty());
    }

    #[test]
    fn region_count_is_bounded_by_conservative_coalescing() {
        let mut region = EffectRegion::empty();
        for index in 0..=MAX_EFFECT_REGION_RECTS {
            region.push(EffectRect::new(index as i32, 0, 1, 1).unwrap());
        }
        assert!(region.rects().len() <= MAX_EFFECT_REGION_RECTS);
        assert!(region.contains_point(0, 0));
        assert!(region.contains_point(MAX_EFFECT_REGION_RECTS as i32, 0));
    }

    #[test]
    fn union_treats_effect_regions_as_geometric_sets() {
        let first = EffectRect::new(10, 20, 30, 40).unwrap();
        let second = EffectRect::new(80, 20, 30, 40).unwrap();
        let region = EffectRegion::from_rect(first);

        assert_eq!(region.union(&region).rects(), &[first]);

        let repeated = region
            .union(&EffectRegion::from_rect(second))
            .union(&region);
        assert_eq!(repeated.rects(), &[first, second]);
    }

    #[test]
    fn bounded_disjointification_preserves_overlapping_region_coverage_once() {
        let first = EffectRect::new(0, 0, 10, 10).unwrap();
        let second = EffectRect::new(5, 0, 10, 10).unwrap();
        let mut region = EffectRegion::from_rect(first);
        region.push(second);

        let result = region.disjoint_bounded();

        assert!(!result.overflowed);
        assert_eq!(result.duplicate_rects_removed, 0);
        assert!(result.overlap_fragments_generated > 0);
        // The horizontal band merges both rects into one exact cover.
        assert_eq!(
            result.region.rects(),
            &[EffectRect::new(0, 0, 15, 10).unwrap()]
        );
        assert!(result.region.contains_point(5, 5));
        assert!(result.region.contains_point(14, 5));
    }

    #[test]
    fn repeated_rectangles_do_not_consume_the_region_bound() {
        let mut region = EffectRegion::empty();
        for index in 0..MAX_EFFECT_REGION_RECTS {
            region.push(EffectRect::new(index as i32 * 2, 0, 1, 1).unwrap());
        }
        let repeated = EffectRegion::from_rect(EffectRect::new(0, 0, 1, 1).unwrap());

        let (result, coalesced) = region.union_with_diagnostics(&repeated);

        assert!(!coalesced);
        assert_eq!(result.rects().len(), MAX_EFFECT_REGION_RECTS);
        assert_eq!(
            result.rects().first(),
            Some(&EffectRect::new(0, 0, 1, 1).unwrap())
        );
    }

    #[test]
    fn rounded_hover_transition_never_leaks_a_corner() {
        let bounds = EffectRect::new(0, 0, 1920, 1080).unwrap();
        let old = rounded_test_region(500, 100, 500, 80, 20, 16);
        let new = rounded_test_region(500, 100, 650, 80, 20, 16);
        let source_damage = old.union(&new);
        assert_eq!(old.rects().len(), 33);
        assert_eq!(new.rects().len(), 33);
        assert_eq!(source_damage.rects().len(), 66);

        let expanded_source = source_damage
            .rects()
            .iter()
            .filter_map(|rect| rect.expanded(12, 12, bounds))
            .collect::<Vec<_>>();
        let pairwise_intersections = expanded_source
            .iter()
            .flat_map(|left| {
                new.rects()
                    .iter()
                    .filter_map(|right| left.intersect(*right))
            })
            .count();
        assert!(pairwise_intersections > MAX_EFFECT_REGION_RECTS);

        let plan = plan_effect_damage(EffectFootprint::symmetric(12), &new, &source_damage, bounds);
        let corner = (500, 100);
        assert!(!new.contains_point(corner.0, corner.1));
        assert!(!plan.output_damage.contains_point(corner.0, corner.1));
        let fallback = plan
            .output_clip_fallback
            .expect("rounded transition should exercise the bounded clip fallback");
        assert_eq!(fallback.input_rect_count, 66);
        assert_eq!(fallback.clip_rect_count, 33);
    }

    #[test]
    fn bounded_intersection_overflow_returns_authoritative_clip() {
        let mut fragmented = EffectRegion::empty();
        for index in 0..MAX_EFFECT_REGION_RECTS {
            fragmented.push(EffectRect::new(0, index as i32, 500 + index as u32, 200).unwrap());
        }
        let mut clip = EffectRegion::from_rect(EffectRect::new(0, 0, 1, 200).unwrap());
        clip.push(EffectRect::new(400, 0, 1, 200).unwrap());

        let result = fragmented.intersect_bounded_within(&clip);

        assert_eq!(result, clip);
        assert!(!result.contains_point(200, 0));
    }

    #[test]
    fn bounded_intersection_reports_overflow_without_bbox_coalescing() {
        let mut fragmented = EffectRegion::empty();
        for index in 0..MAX_EFFECT_REGION_RECTS {
            fragmented.push(EffectRect::new(0, index as i32, 500 + index as u32, 200).unwrap());
        }
        let mut clip = EffectRegion::from_rect(EffectRect::new(0, 0, 1, 200).unwrap());
        clip.push(EffectRect::new(400, 0, 1, 200).unwrap());

        let result = fragmented.intersect_bounded_within_result(&clip);

        assert!(result.overflowed);
        assert_eq!(result.input_rect_count, MAX_EFFECT_REGION_RECTS);
        assert_eq!(result.clip_rect_count, 2);
        assert_eq!(result.region, clip);
    }

    #[test]
    fn internal_work_union_may_coalesce_but_stays_bounded() {
        let mut work = EffectRegion::empty();
        for index in 0..MAX_EFFECT_REGION_RECTS {
            work.push(EffectRect::new(index as i32 * 2, 0, 1, 1).unwrap());
        }
        let additional = EffectRegion::from_rect(
            EffectRect::new(MAX_EFFECT_REGION_RECTS as i32 * 2, 0, 1, 1).unwrap(),
        );

        let (result, coalesced) = work.union_with_diagnostics(&additional);

        assert!(coalesced);
        assert!(result.rects().len() <= MAX_EFFECT_REGION_RECTS);
        assert!(result.contains_point(0, 0));
        assert!(result.contains_point(MAX_EFFECT_REGION_RECTS as i32 * 2, 0));
    }

    #[test]
    fn exact_intersection_preserves_disjoint_region_holes() {
        let mut visible = EffectRegion::from_rect(EffectRect::new(0, 0, 10, 4).unwrap());
        visible.push(EffectRect::new(20, 0, 10, 4).unwrap());
        let source = EffectRegion::from_rect(EffectRect::new(0, 0, 30, 4).unwrap());
        let plan = plan_effect_damage(
            EffectFootprint::ZERO,
            &visible,
            &source,
            EffectRect::new(0, 0, 40, 10).unwrap(),
        );
        assert!(plan.output_damage.contains_point(5, 2));
        assert!(!plan.output_damage.contains_point(15, 2));
        assert!(plan.output_damage.contains_point(25, 2));
    }

    fn assert_region_is_subset_of(region: &EffectRegion, clip: &EffectRegion) {
        assert!(
            region.bounding_rect().is_some(),
            "test region must be bounded"
        );
        for rect in region.rects() {
            for y in rect.y..rect.bottom() {
                for x in rect.x..rect.right() {
                    assert!(
                        clip.contains_point(x, y),
                        "({x}, {y}) escaped the authoritative clip"
                    );
                }
            }
        }
    }

    #[test]
    fn rounded_region_variants_keep_final_damage_inside_authoritative_shape() {
        let bounds = EffectRect::new(0, 0, 800, 300).unwrap();
        let cases = [
            (
                rounded_test_region(100, 60, 500, 80, 20, 16),
                rounded_test_region(100, 60, 500, 80, 20, 16),
            ),
            (
                rounded_test_region(100, 60, 650, 80, 20, 16),
                rounded_test_region(100, 60, 500, 80, 20, 16)
                    .union(&rounded_test_region(100, 60, 650, 80, 20, 16)),
            ),
            (
                rounded_test_region(100, 60, 500, 80, 20, 16),
                rounded_test_region(100, 60, 650, 80, 20, 16)
                    .union(&rounded_test_region(100, 60, 500, 80, 20, 16)),
            ),
            (
                rounded_test_region(100, 60, 500, 80, 28, 16),
                rounded_test_region(100, 60, 500, 80, 20, 16),
            ),
            (
                rounded_test_region(220, 100, 500, 80, 20, 16),
                rounded_test_region(100, 60, 500, 80, 20, 16),
            ),
            (
                rounded_test_region(-10, 60, 500, 80, 20, 16),
                rounded_test_region(-10, 60, 500, 80, 20, 16),
            ),
        ];

        for (visible, source_damage) in cases {
            let plan = plan_effect_damage(
                EffectFootprint::symmetric(12),
                &visible,
                &source_damage,
                bounds,
            );
            assert_region_is_subset_of(&plan.output_damage, &plan.dependency_region);
        }
    }

    #[test]
    fn near_protocol_budget_fragmented_repair_falls_back_to_disconnected_clip() {
        let mut visible = EffectRegion::empty();
        for index in 0..48 {
            visible.push(EffectRect::new(0, index * 2, 80, 1).unwrap());
            visible.push(EffectRect::new(200, index * 2, 80, 1).unwrap());
        }
        let mut fragmented_repair = EffectRegion::empty();
        for index in 0..MAX_EFFECT_REGION_RECTS {
            fragmented_repair
                .push(EffectRect::new(index as i32, 0, 300 - index as u32, 96).unwrap());
        }
        let plan = plan_effect_damage(
            EffectFootprint::ZERO,
            &visible,
            &fragmented_repair,
            EffectRect::new(0, 0, 300, 100).unwrap(),
        );

        assert_eq!(plan.output_damage, visible);
        assert!(!plan.output_damage.contains_point(100, 50));
        assert!(plan.output_damage.contains_point(0, 0));
        assert!(plan.output_damage.contains_point(200, 0));
    }

    #[test]
    fn dependency_influence_reaches_higher_blur_capture_padding() {
        let lower = EffectRegion::from_rect(EffectRect::new(90, 90, 5, 5).unwrap());
        let higher_visible = EffectRegion::from_rect(EffectRect::new(100, 100, 10, 10).unwrap());
        let higher = plan_effect_damage(
            EffectFootprint::symmetric(12),
            &higher_visible,
            &higher_visible,
            EffectRect::new(0, 0, 200, 200).unwrap(),
        );
        assert!(!lower.intersects(&higher_visible));
        assert!(lower.intersects(&higher.capture_region));
    }

    #[test]
    fn subtraction_preserves_transparent_sides() {
        let candidate = EffectRegion::from_rect(EffectRect::new(0, 0, 10, 10).unwrap());
        let opaque = EffectRegion::from_rect(EffectRect::new(2, 2, 6, 6).unwrap());
        let result = candidate.subtract(&opaque);
        assert!(!result.contains_point(5, 5));
        assert!(result.contains_point(1, 5));
        assert!(result.contains_point(8, 5));
        assert!(result.contains_point(5, 1));
        assert!(result.contains_point(5, 8));
    }

    #[test]
    fn subtraction_is_deterministic_for_multiple_opaque_rectangles() {
        let candidate = EffectRegion::from_rect(EffectRect::new(0, 0, 20, 10).unwrap());
        let mut opaque = EffectRegion::from_rect(EffectRect::new(0, 0, 5, 10).unwrap());
        opaque.push(EffectRect::new(10, 0, 5, 10).unwrap());
        assert_eq!(
            candidate.subtract(&opaque).rects(),
            &[
                EffectRect::new(5, 0, 5, 10).unwrap(),
                EffectRect::new(15, 0, 5, 10).unwrap(),
            ]
        );
    }

    #[test]
    fn full_and_empty_exclusion_have_expected_results() {
        let candidate = EffectRegion::from_rect(EffectRect::new(0, 0, 10, 10).unwrap());
        assert!(candidate.subtract(&candidate).is_empty());
        assert_eq!(candidate.subtract(&EffectRegion::empty()), candidate);
    }

    fn test_rects(seed: u64, count: usize) -> Vec<EffectRect> {
        let mut state = seed;
        let mut next = move |modulo: u64| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) % modulo
        };
        (0..count)
            .map(|_| {
                EffectRect::new(
                    next(48) as i32,
                    next(48) as i32,
                    1 + next(24) as u32,
                    1 + next(24) as u32,
                )
                .unwrap()
            })
            .collect()
    }

    #[test]
    fn band_disjointification_matches_coverage_without_overlap() {
        for seed in 0..200 {
            let mut region = EffectRegion::empty();
            for rect in test_rects(seed, 1 + (seed % 12) as usize) {
                region.push(rect);
            }
            let result = region.disjoint_bounded();
            assert!(!result.overflowed);
            let rects = result.region.rects();
            for (index, rect) in rects.iter().enumerate() {
                for other in &rects[index + 1..] {
                    assert!(rect.intersect(*other).is_none(), "seed {seed}");
                }
            }
            for y in -1..74 {
                for x in -1..74 {
                    assert_eq!(
                        result.region.contains_point(x, y),
                        region.contains_point(x, y),
                        "seed {seed} at {x},{y}"
                    );
                }
            }
        }
    }

    #[test]
    fn band_disjointification_keeps_disjoint_input_unchanged() {
        let mut region = EffectRegion::from_rect(EffectRect::new(0, 0, 4, 4).unwrap());
        region.push(EffectRect::new(10, 10, 4, 4).unwrap());
        let result = region.disjoint_bounded();
        assert_eq!(result.region, region);
        assert_eq!(result.overlap_fragments_generated, 0);
    }

    #[test]
    fn push_covering_keeps_coverage_and_drops_nested_rects() {
        for seed in 0..200 {
            let rects = test_rects(seed, 1 + (seed % 16) as usize);
            let mut plain = EffectRegion::empty();
            let mut covering = EffectRegion::empty();
            for rect in &rects {
                plain.push(*rect);
                covering.push_covering(*rect);
            }
            assert!(covering.rects().len() <= plain.rects().len());
            for (index, rect) in covering.rects().iter().enumerate() {
                for (other_index, other) in covering.rects().iter().enumerate() {
                    assert!(index == other_index || !rect.contains_rect(*other));
                }
            }
            for y in -1..74 {
                for x in -1..74 {
                    assert_eq!(covering.contains_point(x, y), plain.contains_point(x, y));
                }
            }
        }
    }
}
