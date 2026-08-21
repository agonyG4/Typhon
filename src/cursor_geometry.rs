use std::fmt;

use wayland_server::protocol::wl_output::Transform;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorSize {
    pub width: u32,
    pub height: u32,
}

impl CursorSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorHotspot {
    pub x: i32,
    pub y: i32,
}

impl CursorHotspot {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorGeometry {
    pub transformed_buffer_size: CursorSize,
    pub logical_size: CursorSize,
    pub logical_hotspot: CursorHotspot,
    pub physical_size: CursorSize,
    pub physical_hotspot: CursorHotspot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorGeometryError {
    ZeroDimension,
    ZeroBufferScale,
    NonDivisibleBufferSize,
    HotspotOutOfBounds,
    UnsupportedTransform,
}

impl fmt::Display for CursorGeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ZeroDimension => "cursor geometry has a zero dimension",
            Self::ZeroBufferScale => "cursor buffer scale must be nonzero",
            Self::NonDivisibleBufferSize => {
                "cursor buffer dimensions are not divisible by buffer scale"
            }
            Self::HotspotOutOfBounds => "cursor hotspot is outside the cursor surface",
            Self::UnsupportedTransform => "cursor buffer transform is unsupported",
        })
    }
}

impl std::error::Error for CursorGeometryError {}

pub fn logical_size(
    buffer_width: u32,
    buffer_height: u32,
    buffer_scale: u32,
    transform: Transform,
) -> Result<CursorSize, CursorGeometryError> {
    if buffer_width == 0 || buffer_height == 0 {
        return Err(CursorGeometryError::ZeroDimension);
    }
    if buffer_scale == 0 {
        return Err(CursorGeometryError::ZeroBufferScale);
    }
    let transformed = transformed_size(CursorSize::new(buffer_width, buffer_height), transform)?;
    if transformed.width % buffer_scale != 0 || transformed.height % buffer_scale != 0 {
        return Err(CursorGeometryError::NonDivisibleBufferSize);
    }
    Ok(CursorSize::new(
        transformed.width / buffer_scale,
        transformed.height / buffer_scale,
    ))
}

pub fn physical_size(logical: CursorSize, output_scale: f64) -> CursorSize {
    CursorSize::new(
        scale_extent(logical.width, output_scale),
        scale_extent(logical.height, output_scale),
    )
}

pub fn transform_hotspot(
    hotspot: CursorHotspot,
    source: CursorSize,
    transform: Transform,
) -> Result<CursorHotspot, CursorGeometryError> {
    validate_hotspot(hotspot, source)?;
    let x = i64::from(hotspot.x);
    let y = i64::from(hotspot.y);
    let width = i64::from(source.width);
    let height = i64::from(source.height);
    let (x, y) = match transform {
        Transform::Normal => (x, y),
        Transform::_90 => (height - 1 - y, x),
        Transform::_180 => (width - 1 - x, height - 1 - y),
        Transform::_270 => (y, width - 1 - x),
        Transform::Flipped => (width - 1 - x, y),
        Transform::Flipped90 => (y, x),
        Transform::Flipped180 => (x, height - 1 - y),
        Transform::Flipped270 => (height - 1 - y, width - 1 - x),
        _ => return Err(CursorGeometryError::UnsupportedTransform),
    };
    Ok(CursorHotspot::new(
        i32::try_from(x).map_err(|_| CursorGeometryError::HotspotOutOfBounds)?,
        i32::try_from(y).map_err(|_| CursorGeometryError::HotspotOutOfBounds)?,
    ))
}

pub fn geometry_for_surface(
    buffer: CursorSize,
    buffer_scale: u32,
    transform: Transform,
    viewport_destination: Option<CursorSize>,
    hotspot: CursorHotspot,
    output_scale: f64,
) -> Result<CursorGeometry, CursorGeometryError> {
    let transformed_buffer_size = transformed_size(buffer, transform)?;
    let committed_logical_size =
        logical_size(buffer.width, buffer.height, buffer_scale, transform)?;
    let logical_size = viewport_destination.unwrap_or(committed_logical_size);
    if logical_size.width == 0 || logical_size.height == 0 {
        return Err(CursorGeometryError::ZeroDimension);
    }
    validate_hotspot(hotspot, logical_size)?;
    let physical_size = physical_size(logical_size, output_scale);
    let physical_hotspot = CursorHotspot::new(
        scale_coordinate(hotspot.x, output_scale),
        scale_coordinate(hotspot.y, output_scale),
    );
    Ok(CursorGeometry {
        transformed_buffer_size,
        logical_size,
        logical_hotspot: hotspot,
        physical_size,
        physical_hotspot,
    })
}

fn transformed_size(
    size: CursorSize,
    transform: Transform,
) -> Result<CursorSize, CursorGeometryError> {
    if size.width == 0 || size.height == 0 {
        return Err(CursorGeometryError::ZeroDimension);
    }
    match transform {
        Transform::Normal | Transform::_180 | Transform::Flipped | Transform::Flipped180 => {
            Ok(size)
        }
        Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270 => {
            Ok(CursorSize::new(size.height, size.width))
        }
        _ => Err(CursorGeometryError::UnsupportedTransform),
    }
}

fn validate_hotspot(hotspot: CursorHotspot, size: CursorSize) -> Result<(), CursorGeometryError> {
    if hotspot.x < 0
        || hotspot.y < 0
        || u32::try_from(hotspot.x)
            .ok()
            .is_none_or(|x| x >= size.width)
        || u32::try_from(hotspot.y)
            .ok()
            .is_none_or(|y| y >= size.height)
    {
        Err(CursorGeometryError::HotspotOutOfBounds)
    } else {
        Ok(())
    }
}

fn scale_extent(value: u32, output_scale: f64) -> u32 {
    if value == 0 {
        return 0;
    }
    let scale = if output_scale.is_finite() && output_scale > 0.0 {
        output_scale
    } else {
        1.0
    };
    (f64::from(value) * scale)
        .round()
        .clamp(1.0, f64::from(u32::MAX)) as u32
}

fn scale_coordinate(value: i32, output_scale: f64) -> i32 {
    let scale = if output_scale.is_finite() && output_scale > 0.0 {
        output_scale
    } else {
        1.0
    };
    (f64::from(value) * scale)
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::{
        CursorHotspot as Hotspot, CursorSize as Size, geometry_for_surface, logical_size,
        physical_size, transform_hotspot,
    };
    use wayland_server::protocol::wl_output::Transform;

    #[test]
    fn buffer_scale_one_preserves_logical_size() {
        assert_eq!(
            logical_size(24, 24, 1, Transform::Normal).unwrap(),
            Size::new(24, 24)
        );
    }

    #[test]
    fn integer_buffer_scale_converts_pixels_to_logical_size() {
        assert_eq!(
            logical_size(48, 48, 2, Transform::Normal).unwrap(),
            Size::new(24, 24)
        );
    }

    #[test]
    fn non_square_buffer_scale_preserves_aspect() {
        assert_eq!(
            logical_size(64, 32, 2, Transform::Normal).unwrap(),
            Size::new(32, 16)
        );
    }

    #[test]
    fn transform_rotations_swap_dimensions_and_transform_hotspot() {
        let source = Size::new(4, 3);
        let hotspot = Hotspot::new(1, 2);

        assert_eq!(logical_size(4, 3, 1, Transform::Normal).unwrap(), source);
        assert_eq!(
            logical_size(4, 3, 1, Transform::_90).unwrap(),
            Size::new(3, 4)
        );
        assert_eq!(logical_size(4, 3, 1, Transform::_180).unwrap(), source);
        assert_eq!(
            logical_size(4, 3, 1, Transform::_270).unwrap(),
            Size::new(3, 4)
        );
        assert_eq!(logical_size(4, 3, 1, Transform::Flipped).unwrap(), source);
        assert_eq!(
            logical_size(4, 3, 1, Transform::Flipped90).unwrap(),
            Size::new(3, 4)
        );
        assert_eq!(
            logical_size(4, 3, 1, Transform::Flipped180).unwrap(),
            source
        );
        assert_eq!(
            logical_size(4, 3, 1, Transform::Flipped270).unwrap(),
            Size::new(3, 4)
        );

        assert_eq!(
            transform_hotspot(hotspot, source, Transform::Normal).unwrap(),
            hotspot
        );
        assert_eq!(
            transform_hotspot(hotspot, source, Transform::_90).unwrap(),
            Hotspot::new(0, 1)
        );
        assert_eq!(
            transform_hotspot(hotspot, source, Transform::_180).unwrap(),
            Hotspot::new(2, 0)
        );
        assert_eq!(
            transform_hotspot(hotspot, source, Transform::_270).unwrap(),
            Hotspot::new(2, 2)
        );
        assert_eq!(
            transform_hotspot(hotspot, source, Transform::Flipped).unwrap(),
            Hotspot::new(2, 2)
        );
        assert_eq!(
            transform_hotspot(hotspot, source, Transform::Flipped90).unwrap(),
            Hotspot::new(2, 1)
        );
        assert_eq!(
            transform_hotspot(hotspot, source, Transform::Flipped180).unwrap(),
            Hotspot::new(1, 0)
        );
        assert_eq!(
            transform_hotspot(hotspot, source, Transform::Flipped270).unwrap(),
            Hotspot::new(0, 2)
        );
    }

    #[test]
    fn fractional_output_scale_is_applied_once() {
        assert_eq!(physical_size(Size::new(24, 24), 1.5), Size::new(36, 36));
    }

    #[test]
    fn hardware_and_software_cursor_geometry_have_equal_visual_bounds() {
        let geometry = geometry_for_surface(
            Size::new(48, 48),
            2,
            Transform::Normal,
            None,
            Hotspot::new(12, 12),
            1.5,
        )
        .unwrap();

        assert_eq!(geometry.logical_size, Size::new(24, 24));
        assert_eq!(geometry.physical_size, Size::new(36, 36));
        assert_eq!(geometry.logical_hotspot, Hotspot::new(12, 12));
        assert_eq!(geometry.physical_hotspot, Hotspot::new(18, 18));
        assert_ne!(geometry.physical_size, Size::new(72, 72));
    }

    #[test]
    fn geometry_rejects_invalid_scale_and_hotspot() {
        assert!(logical_size(24, 24, 0, Transform::Normal).is_err());
        assert!(logical_size(25, 24, 2, Transform::Normal).is_err());
        assert!(transform_hotspot(Hotspot::new(4, 0), Size::new(4, 3), Transform::Normal).is_err());
    }
}
