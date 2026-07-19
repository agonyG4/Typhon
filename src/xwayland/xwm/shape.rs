//! Bounded Shape extension normalization.

pub(crate) const MAX_SHAPE_RECTS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShapeRect {
    pub(crate) x: i16,
    pub(crate) y: i16,
    pub(crate) width: u16,
    pub(crate) height: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShapeRegion {
    pub(crate) rectangles: Vec<ShapeRect>,
    pub(crate) fallback_rectangular: bool,
}

pub(crate) fn normalize_region(rectangles: &[ShapeRect], fallback: ShapeRect) -> ShapeRegion {
    if rectangles.is_empty() {
        return ShapeRegion {
            rectangles: vec![fallback],
            fallback_rectangular: true,
        };
    }
    let bounded = rectangles
        .iter()
        .copied()
        .take(MAX_SHAPE_RECTS)
        .collect::<Vec<_>>();
    ShapeRegion {
        rectangles: bounded,
        fallback_rectangular: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_shape_uses_rectangular_fallback() {
        let fallback = ShapeRect {
            x: 0,
            y: 0,
            width: 10,
            height: 20,
        };
        let region = normalize_region(&[], fallback);
        assert!(region.fallback_rectangular);
        assert_eq!(region.rectangles, vec![fallback]);
    }

    #[test]
    fn shape_region_is_bounded() {
        let rect = ShapeRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let input = vec![rect; MAX_SHAPE_RECTS + 1];
        assert_eq!(
            normalize_region(&input, rect).rectangles.len(),
            MAX_SHAPE_RECTS
        );
    }
}
