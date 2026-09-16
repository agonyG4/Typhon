//! The single coordinate model for a wl_surface buffer.
//!
//! Wayland viewport source coordinates are measured after the buffer transform
//! and buffer scale. This module keeps that fact in one immutable value that
//! can be used by validation, rendering, and damage projection.

use crate::render_backend::buffer::BufferSize;
use wayland_server::protocol::wl_output::Transform;

use super::SurfaceDamageRect;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceGeometryRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl SurfaceGeometryRect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceUvQuad {
    pub top_left: [f32; 2],
    pub bottom_left: [f32; 2],
    pub bottom_right: [f32; 2],
    pub top_right: [f32; 2],
}

impl SurfaceUvQuad {
    pub const FULL: Self = Self {
        top_left: [0.0, 0.0],
        bottom_left: [0.0, 1.0],
        bottom_right: [1.0, 1.0],
        top_right: [1.0, 0.0],
    };

    pub fn sample(self, u: f32, v: f32) -> [f32; 2] {
        let top = lerp(self.top_left, self.top_right, u);
        let bottom = lerp(self.bottom_left, self.bottom_right, u);
        lerp(top, bottom, v)
    }

    pub fn sub_quad(self, left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            top_left: self.sample(left, top),
            bottom_left: self.sample(left, bottom),
            bottom_right: self.sample(right, bottom),
            top_right: self.sample(right, top),
        }
    }
}

fn lerp(first: [f32; 2], second: [f32; 2], amount: f32) -> [f32; 2] {
    [
        first[0] + (second[0] - first[0]) * amount,
        first[1] + (second[1] - first[1]) * amount,
    ]
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceBufferMapping {
    raw_size: BufferSize,
    transformed_size: BufferSize,
    logical_extent: BufferSize,
    surface_extent: BufferSize,
    buffer_scale: u32,
    buffer_transform: Transform,
    source: SurfaceGeometryRect,
    destination: Option<BufferSize>,
}

#[allow(dead_code)]
impl SurfaceBufferMapping {
    pub fn new(
        raw_size: BufferSize,
        buffer_scale: u32,
        buffer_transform: Transform,
        source: Option<SurfaceGeometryRect>,
        destination: Option<BufferSize>,
    ) -> Option<Self> {
        if raw_size.width == 0 || raw_size.height == 0 || buffer_scale == 0 {
            return None;
        }
        let transformed_size = transformed_buffer_size(raw_size, buffer_transform)?;
        if transformed_size.width % buffer_scale != 0 || transformed_size.height % buffer_scale != 0
        {
            return None;
        }
        let logical_extent = BufferSize::new(
            transformed_size.width / buffer_scale,
            transformed_size.height / buffer_scale,
        )?;
        let full_source = SurfaceGeometryRect::new(
            0.0,
            0.0,
            f64::from(logical_extent.width),
            f64::from(logical_extent.height),
        );
        let source = source.unwrap_or(full_source);
        if !valid_source(source, logical_extent) {
            return None;
        }
        if destination.is_some_and(|destination| destination.width == 0 || destination.height == 0)
        {
            return None;
        }
        let surface_extent = match destination {
            Some(destination) => destination,
            None => BufferSize::new(ceil_extent(source.width)?, ceil_extent(source.height)?)?,
        };
        Some(Self {
            raw_size,
            transformed_size,
            logical_extent,
            surface_extent,
            buffer_scale,
            buffer_transform,
            source,
            destination,
        })
    }

    pub const fn raw_size(self) -> BufferSize {
        self.raw_size
    }

    pub const fn transformed_size(self) -> BufferSize {
        self.transformed_size
    }

    pub const fn logical_extent(self) -> BufferSize {
        self.logical_extent
    }

    pub const fn surface_extent(self) -> BufferSize {
        self.surface_extent
    }

    pub const fn buffer_scale(self) -> u32 {
        self.buffer_scale
    }

    pub const fn buffer_transform(self) -> Transform {
        self.buffer_transform
    }

    pub fn is_identity(self) -> bool {
        self.buffer_scale == 1
            && self.buffer_transform == Transform::Normal
            && self.destination.is_none()
            && self.source.x == 0.0
            && self.source.y == 0.0
            && self.source.width == self.logical_extent.width as f64
            && self.source.height == self.logical_extent.height as f64
    }

    pub fn source_uv_quad(self) -> Option<SurfaceUvQuad> {
        let width = f64::from(self.surface_extent.width);
        let height = f64::from(self.surface_extent.height);
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        let corners = [(0.0, 0.0), (0.0, height), (width, height), (width, 0.0)];
        let mut uv = [[0.0_f32; 2]; 4];
        for (index, (x, y)) in corners.into_iter().enumerate() {
            let (raw_x, raw_y) = self.surface_point_to_raw(x, y)?;
            uv[index] = [
                unit_f32(raw_x / f64::from(self.raw_size.width))?,
                unit_f32(raw_y / f64::from(self.raw_size.height))?,
            ];
        }
        Some(SurfaceUvQuad {
            top_left: uv[0],
            bottom_left: uv[1],
            bottom_right: uv[2],
            top_right: uv[3],
        })
    }

    /// Convert a final surface-local half-open rectangle into raw pixels.
    /// `None` means the geometry could not be represented safely; an inner
    /// `None` means the clipped rectangle is empty.
    pub fn map_surface_rect_to_buffer(
        self,
        rect: SurfaceDamageRect,
    ) -> Option<Option<SurfaceDamageRect>> {
        let right = u64::from(rect.x).checked_add(u64::from(rect.width))?;
        let bottom = u64::from(rect.y).checked_add(u64::from(rect.height))?;
        let left = f64::from(rect.x).clamp(0.0, f64::from(self.surface_extent.width));
        let top = f64::from(rect.y).clamp(0.0, f64::from(self.surface_extent.height));
        let right = (right as f64).clamp(0.0, f64::from(self.surface_extent.width));
        let bottom = (bottom as f64).clamp(0.0, f64::from(self.surface_extent.height));
        if right <= left || bottom <= top {
            return Some(None);
        }
        let points = [
            self.surface_point_to_raw(left, top)?,
            self.surface_point_to_raw(left, bottom)?,
            self.surface_point_to_raw(right, bottom)?,
            self.surface_point_to_raw(right, top)?,
        ];
        Some(round_and_clip_points(
            points,
            self.raw_size.width,
            self.raw_size.height,
        ))
    }

    /// Project raw canonical damage into final surface-local coordinates.
    /// The crop is applied in post-transform/post-scale coordinates before the
    /// result is rounded, so raw pixels outside a viewport source contribute
    /// no output damage.
    pub fn map_buffer_rect_to_surface(
        self,
        rect: SurfaceDamageRect,
    ) -> Option<Option<SurfaceDamageRect>> {
        let right = u64::from(rect.x).checked_add(u64::from(rect.width))?;
        let bottom = u64::from(rect.y).checked_add(u64::from(rect.height))?;
        let left = f64::from(rect.x).clamp(0.0, f64::from(self.raw_size.width));
        let top = f64::from(rect.y).clamp(0.0, f64::from(self.raw_size.height));
        let right = (right as f64).clamp(0.0, f64::from(self.raw_size.width));
        let bottom = (bottom as f64).clamp(0.0, f64::from(self.raw_size.height));
        if right <= left || bottom <= top {
            return Some(None);
        }
        let transformed = [
            self.raw_point_to_logical(left, top)?,
            self.raw_point_to_logical(left, bottom)?,
            self.raw_point_to_logical(right, bottom)?,
            self.raw_point_to_logical(right, top)?,
        ];
        let left = transformed
            .iter()
            .map(|point| point.0)
            .fold(f64::INFINITY, f64::min)
            .max(self.source.x);
        let top = transformed
            .iter()
            .map(|point| point.1)
            .fold(f64::INFINITY, f64::min)
            .max(self.source.y);
        let right = transformed
            .iter()
            .map(|point| point.0)
            .fold(f64::NEG_INFINITY, f64::max)
            .min(self.source.x + self.source.width);
        let bottom = transformed
            .iter()
            .map(|point| point.1)
            .fold(f64::NEG_INFINITY, f64::max)
            .min(self.source.y + self.source.height);
        if right <= left || bottom <= top {
            return Some(None);
        }
        let (surface_left, surface_top) = self.logical_point_to_surface(left, top)?;
        let (surface_right, surface_bottom) = self.logical_point_to_surface(right, bottom)?;
        Some(round_and_clip_points(
            [
                (surface_left, surface_top),
                (surface_left, surface_bottom),
                (surface_right, surface_bottom),
                (surface_right, surface_top),
            ],
            self.surface_extent.width,
            self.surface_extent.height,
        ))
    }

    fn surface_point_to_raw(self, x: f64, y: f64) -> Option<(f64, f64)> {
        let (logical_x, logical_y) = self.surface_point_to_logical(x, y)?;
        self.transformed_point_to_raw(
            logical_x * f64::from(self.buffer_scale),
            logical_y * f64::from(self.buffer_scale),
        )
    }

    fn surface_point_to_logical(self, x: f64, y: f64) -> Option<(f64, f64)> {
        let (destination_width, destination_height) = self
            .destination
            .map(|destination| (f64::from(destination.width), f64::from(destination.height)))
            .unwrap_or((self.source.width, self.source.height));
        if destination_width <= 0.0 || destination_height <= 0.0 {
            return None;
        }
        Some((
            self.source.x + x * self.source.width / destination_width,
            self.source.y + y * self.source.height / destination_height,
        ))
    }

    fn logical_point_to_surface(self, x: f64, y: f64) -> Option<(f64, f64)> {
        let (destination_width, destination_height) = self
            .destination
            .map(|destination| (f64::from(destination.width), f64::from(destination.height)))
            .unwrap_or((
                f64::from(self.surface_extent.width),
                f64::from(self.surface_extent.height),
            ));
        if self.source.width <= 0.0 || self.source.height <= 0.0 {
            return None;
        }
        Some((
            (x - self.source.x) * destination_width / self.source.width,
            (y - self.source.y) * destination_height / self.source.height,
        ))
    }

    fn raw_point_to_logical(self, x: f64, y: f64) -> Option<(f64, f64)> {
        let (transformed_x, transformed_y) = self.raw_point_to_transformed(x, y)?;
        Some((
            transformed_x / f64::from(self.buffer_scale),
            transformed_y / f64::from(self.buffer_scale),
        ))
    }

    fn raw_point_to_transformed(self, x: f64, y: f64) -> Option<(f64, f64)> {
        transform_raw_point(
            x,
            y,
            self.raw_size.width as f64,
            self.raw_size.height as f64,
            self.buffer_transform,
        )
    }

    fn transformed_point_to_raw(self, x: f64, y: f64) -> Option<(f64, f64)> {
        inverse_transform_point(
            x,
            y,
            self.raw_size.width as f64,
            self.raw_size.height as f64,
            self.buffer_transform,
        )
    }
}

pub fn transformed_buffer_size(size: BufferSize, transform: Transform) -> Option<BufferSize> {
    match transform {
        Transform::Normal | Transform::_180 | Transform::Flipped | Transform::Flipped180 => {
            Some(size)
        }
        Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270 => {
            BufferSize::new(size.height, size.width)
        }
        _ => None,
    }
}

/// Return the transformed location of one raw-buffer pixel. This is shared
/// with the cursor raster path so ordinary surfaces and cursors use the same
/// Wayland orientation table; cursor hotspot policy remains separate.
pub fn transform_buffer_pixel(
    x: u32,
    y: u32,
    size: BufferSize,
    transform: Transform,
) -> Option<(u32, u32)> {
    if x >= size.width || y >= size.height {
        return None;
    }
    let (x, y) = transform_raw_point(
        f64::from(x) + 0.5,
        f64::from(y) + 0.5,
        f64::from(size.width),
        f64::from(size.height),
        transform,
    )?;
    let x = x.floor();
    let y = y.floor();
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
        return None;
    }
    let transformed = transformed_buffer_size(size, transform)?;
    let x = u32::try_from(x as u64).ok()?;
    let y = u32::try_from(y as u64).ok()?;
    (x < transformed.width && y < transformed.height).then_some((x, y))
}

fn valid_source(source: SurfaceGeometryRect, logical_extent: BufferSize) -> bool {
    const TOLERANCE: f64 = 1.0 / 256.0;
    source.x.is_finite()
        && source.y.is_finite()
        && source.width.is_finite()
        && source.height.is_finite()
        && source.x >= 0.0
        && source.y >= 0.0
        && source.width > 0.0
        && source.height > 0.0
        && source.x + source.width <= f64::from(logical_extent.width) + TOLERANCE
        && source.y + source.height <= f64::from(logical_extent.height) + TOLERANCE
}

fn ceil_extent(value: f64) -> Option<u32> {
    if !value.is_finite() || value <= 0.0 || value > f64::from(u32::MAX) {
        return None;
    }
    Some(value.ceil() as u32)
}

fn round_and_clip_points(
    points: [(f64, f64); 4],
    width: u32,
    height: u32,
) -> Option<SurfaceDamageRect> {
    if points.iter().any(|(x, y)| !x.is_finite() || !y.is_finite()) {
        return None;
    }
    let min_x = points
        .iter()
        .map(|point| point.0)
        .fold(f64::INFINITY, f64::min)
        .clamp(0.0, f64::from(width));
    let min_y = points
        .iter()
        .map(|point| point.1)
        .fold(f64::INFINITY, f64::min)
        .clamp(0.0, f64::from(height));
    let max_x = points
        .iter()
        .map(|point| point.0)
        .fold(f64::NEG_INFINITY, f64::max)
        .clamp(0.0, f64::from(width));
    let max_y = points
        .iter()
        .map(|point| point.1)
        .fold(f64::NEG_INFINITY, f64::max)
        .clamp(0.0, f64::from(height));
    if max_x <= min_x || max_y <= min_y {
        return None;
    }
    let x = floor_u32(min_x)?;
    let y = floor_u32(min_y)?;
    let right = ceil_u32(max_x)?.min(width);
    let bottom = ceil_u32(max_y)?.min(height);
    (right > x && bottom > y).then_some(SurfaceDamageRect {
        x,
        y,
        width: right - x,
        height: bottom - y,
    })
}

fn floor_u32(value: f64) -> Option<u32> {
    (value.is_finite() && value >= 0.0 && value <= f64::from(u32::MAX))
        .then_some(value.floor() as u32)
}

fn ceil_u32(value: f64) -> Option<u32> {
    (value.is_finite() && value >= 0.0 && value <= f64::from(u32::MAX))
        .then_some(value.ceil() as u32)
}

fn unit_f32(value: f64) -> Option<f32> {
    (value.is_finite() && (0.0..=1.0).contains(&value)).then_some(value as f32)
}

fn transform_raw_point(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    transform: Transform,
) -> Option<(f64, f64)> {
    Some(match transform {
        Transform::Normal => (x, y),
        Transform::_90 => (height - y, x),
        Transform::_180 => (width - x, height - y),
        Transform::_270 => (y, width - x),
        Transform::Flipped => (width - x, y),
        Transform::Flipped90 => (y, x),
        Transform::Flipped180 => (x, height - y),
        Transform::Flipped270 => (height - y, width - x),
        _ => return None,
    })
}

fn inverse_transform_point(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    transform: Transform,
) -> Option<(f64, f64)> {
    Some(match transform {
        Transform::Normal => (x, y),
        Transform::_90 => (y, height - x),
        Transform::_180 => (width - x, height - y),
        Transform::_270 => (width - y, x),
        Transform::Flipped => (width - x, y),
        Transform::Flipped90 => (y, x),
        Transform::Flipped180 => (x, height - y),
        Transform::Flipped270 => (width - y, height - x),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(width: u32, height: u32) -> BufferSize {
        BufferSize::new(width, height).unwrap()
    }

    #[test]
    fn asymmetric_transform_uvs_cover_all_eight_orientations() {
        let transforms = [
            Transform::Normal,
            Transform::_90,
            Transform::_180,
            Transform::_270,
            Transform::Flipped,
            Transform::Flipped90,
            Transform::Flipped180,
            Transform::Flipped270,
        ];
        for transform in transforms {
            let mapping = SurfaceBufferMapping::new(size(3, 2), 1, transform, None, None).unwrap();
            let quad = mapping.source_uv_quad().unwrap();
            assert!(
                quad.top_left
                    .iter()
                    .chain(quad.bottom_left.iter())
                    .chain(quad.bottom_right.iter())
                    .chain(quad.top_right.iter())
                    .all(|value| (0.0..=1.0).contains(value)),
                "{transform:?}"
            );
            assert_eq!(mapping.surface_extent(), mapping.transformed_size());
        }
    }

    #[test]
    fn asymmetric_transform_pixels_match_the_shared_orientation_table() {
        let pixels = [0, 1, 2, 3, 4, 5];
        let cases = [
            (Transform::Normal, 3, 2, [0, 1, 2, 3, 4, 5]),
            (Transform::_90, 2, 3, [3, 0, 4, 1, 5, 2]),
            (Transform::_180, 3, 2, [5, 4, 3, 2, 1, 0]),
            (Transform::_270, 2, 3, [2, 5, 1, 4, 0, 3]),
            (Transform::Flipped, 3, 2, [2, 1, 0, 5, 4, 3]),
            (Transform::Flipped90, 2, 3, [0, 3, 1, 4, 2, 5]),
            (Transform::Flipped180, 3, 2, [3, 4, 5, 0, 1, 2]),
            (Transform::Flipped270, 2, 3, [5, 2, 4, 1, 3, 0]),
        ];
        let raw_size = size(3, 2);
        for (transform, width, height, expected) in cases {
            let mut output = vec![u32::MAX; (width * height) as usize];
            for y in 0..raw_size.height {
                for x in 0..raw_size.width {
                    let (output_x, output_y) =
                        transform_buffer_pixel(x, y, raw_size, transform).unwrap();
                    output[(output_y * width + output_x) as usize] = pixels[(y * 3 + x) as usize];
                }
            }
            assert_eq!(output, expected, "transform {transform:?}");
        }
    }

    #[test]
    fn source_is_in_post_transform_and_post_scale_space() {
        let mapping = SurfaceBufferMapping::new(
            size(2, 4),
            1,
            Transform::_90,
            Some(SurfaceGeometryRect::new(2.0, 0.0, 2.0, 2.0)),
            None,
        )
        .unwrap();
        assert_eq!(mapping.surface_extent(), size(2, 2));
        assert_eq!(mapping.source_uv_quad().unwrap().top_left, [0.0, 0.5]);
    }

    #[test]
    fn surface_damage_rounds_outward_and_clips_to_raw_buffer() {
        let mapping =
            SurfaceBufferMapping::new(size(200, 100), 2, Transform::Normal, None, None).unwrap();
        assert_eq!(
            mapping
                .map_surface_rect_to_buffer(SurfaceDamageRect {
                    x: 2,
                    y: 3,
                    width: 4,
                    height: 5,
                })
                .unwrap(),
            Some(SurfaceDamageRect {
                x: 4,
                y: 6,
                width: 8,
                height: 10,
            })
        );
    }

    #[test]
    fn raw_and_surface_damage_mapping_agrees_for_all_transforms() {
        let cases = [
            (Transform::Normal, (0, 0)),
            (Transform::_90, (1, 0)),
            (Transform::_180, (2, 1)),
            (Transform::_270, (0, 2)),
            (Transform::Flipped, (2, 0)),
            (Transform::Flipped90, (0, 0)),
            (Transform::Flipped180, (0, 1)),
            (Transform::Flipped270, (1, 2)),
        ];
        for (transform, expected_origin) in cases {
            let mapping = SurfaceBufferMapping::new(size(3, 2), 1, transform, None, None).unwrap();
            let surface = mapping
                .map_buffer_rect_to_surface(SurfaceDamageRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                })
                .unwrap()
                .unwrap();
            assert_eq!((surface.x, surface.y), expected_origin, "{transform:?}");
            assert_eq!((surface.width, surface.height), (1, 1), "{transform:?}");
            let raw = mapping
                .map_surface_rect_to_buffer(surface)
                .unwrap()
                .unwrap();
            assert_eq!(
                raw,
                SurfaceDamageRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1
                }
            );
        }
    }

    #[test]
    fn off_center_damage_maps_conservatively_for_all_transforms() {
        let cases = [
            (Transform::Normal, (0, 0, 2, 1)),
            (Transform::_90, (1, 0, 1, 2)),
            (Transform::_180, (1, 1, 2, 1)),
            (Transform::_270, (0, 1, 1, 2)),
            (Transform::Flipped, (1, 0, 2, 1)),
            (Transform::Flipped90, (0, 0, 1, 2)),
            (Transform::Flipped180, (0, 1, 2, 1)),
            (Transform::Flipped270, (1, 1, 1, 2)),
        ];
        for (transform, (x, y, width, height)) in cases {
            let mapping = SurfaceBufferMapping::new(size(3, 2), 1, transform, None, None).unwrap();
            assert_eq!(
                mapping
                    .map_buffer_rect_to_surface(SurfaceDamageRect {
                        x: 0,
                        y: 0,
                        width: 2,
                        height: 1,
                    })
                    .unwrap(),
                Some(SurfaceDamageRect {
                    x,
                    y,
                    width,
                    height,
                }),
                "transform {transform:?}"
            );
        }
    }

    #[test]
    fn mapping_composes_transform_scale_source_and_destination() {
        let mapping = SurfaceBufferMapping::new(
            size(4, 6),
            2,
            Transform::_90,
            Some(SurfaceGeometryRect::new(1.0, 0.0, 2.0, 2.0)),
            Some(size(4, 6)),
        )
        .unwrap();

        assert_eq!(mapping.logical_extent(), size(3, 2));
        assert_eq!(mapping.surface_extent(), size(4, 6));
        assert_eq!(
            mapping.source_uv_quad().unwrap(),
            SurfaceUvQuad {
                top_left: [0.0, 2.0 / 3.0],
                bottom_left: [1.0, 2.0 / 3.0],
                bottom_right: [1.0, 0.0],
                top_right: [0.0, 0.0],
            }
        );
    }
}
