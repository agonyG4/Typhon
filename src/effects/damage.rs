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

    fn intersect(self, other: Self) -> Option<Self> {
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

    fn union(self, other: Self) -> Option<Self> {
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

    fn expand_clamped_xy(&self, radius_x: u32, radius_y: u32, bounds: EffectRect) -> Self {
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectDamagePlan {
    pub output_damage: EffectRegion,
    pub source_query_region: EffectRegion,
    pub dependency_region: EffectRegion,
    pub capture_region: EffectRegion,
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
    let output_damage = source_damage
        .expand_clamped_xy(
            footprint.sample_radius_x,
            footprint.sample_radius_y,
            output_bounds,
        )
        .intersect_rect(visible_region.bounding_rect().unwrap_or(output_bounds));
    let dependency_region = output_damage.expand_clamped_xy(
        footprint.sample_radius_x,
        footprint.sample_radius_y,
        output_bounds,
    );
    EffectDamagePlan {
        output_damage,
        source_query_region: capture_region.clone(),
        dependency_region,
        capture_region,
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
    fn blur_dependency_region_expands_requested_output() {
        let footprint = EffectFootprint::symmetric(8);
        let visible = EffectRegion::from_rect(EffectRect::new(100, 100, 100, 40).unwrap());
        let source = visible.clone();
        let plan = plan_effect_damage(
            footprint,
            &visible,
            &source,
            EffectRect::new(0, 0, 1920, 1080).unwrap(),
        );
        assert!(plan.dependency_region.contains_point(92, 92));
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
}
