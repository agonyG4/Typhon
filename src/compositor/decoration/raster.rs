use std::sync::Arc;

const MAX_RASTER_DIMENSION: u32 = 256;
const MAX_RASTER_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecorationRasterAsset {
    asset_id: u64,
    width: u32,
    height: u32,
    rgba_premultiplied: Arc<[u8]>,
}

impl DecorationRasterAsset {
    pub(crate) fn from_pixels(
        asset_id: u64,
        width: u32,
        height: u32,
        rgba_premultiplied: Arc<[u8]>,
    ) -> Self {
        Self {
            asset_id,
            width,
            height,
            rgba_premultiplied,
        }
    }

    pub fn asset_id(&self) -> u64 {
        self.asset_id
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn rgba_premultiplied(&self) -> &[u8] {
        &self.rgba_premultiplied
    }
}

pub(crate) fn rasterize_svg(
    path: &str,
    bytes: &[u8],
    scale: f64,
) -> Result<DecorationRasterAsset, String> {
    if !(scale.is_finite() && scale > 0.0 && scale <= 4.0) {
        return Err(format!("invalid raster scale for {path}: {scale}"));
    }
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(bytes, &options)
        .map_err(|error| format!("SVG parse failed for {path}: {error}"))?;
    let source_size = tree.size();
    let width = f64::from(source_size.width())
        .mul_add(scale, 0.999_999)
        .floor() as u32;
    let height = f64::from(source_size.height())
        .mul_add(scale, 0.999_999)
        .floor() as u32;
    if width == 0 || height == 0 || width > MAX_RASTER_DIMENSION || height > MAX_RASTER_DIMENSION {
        return Err(format!(
            "SVG raster dimensions are outside bounds for {path}"
        ));
    }
    let byte_len = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| format!("SVG raster size overflows for {path}"))?;
    if byte_len > MAX_RASTER_BYTES {
        return Err(format!("SVG raster exceeds bounds for {path}"));
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| format!("could not allocate SVG raster for {path}"))?;
    let transform = resvg::tiny_skia::Transform::from_scale(scale as f32, scale as f32);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let pixels = Arc::<[u8]>::from(pixmap.data());
    let asset_id = asset_id(path, scale, &pixels);
    Ok(DecorationRasterAsset {
        asset_id,
        width,
        height,
        rgba_premultiplied: pixels,
    })
}

fn asset_id(path: &str, scale: f64, pixels: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in path
        .as_bytes()
        .iter()
        .copied()
        .chain(scale.to_bits().to_le_bytes())
        .chain(pixels.iter().copied())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::rasterize_svg;

    const ICON: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6" fill="#e9524a"/></svg>"##;

    #[test]
    fn rasterizes_bounded_svg_with_transparent_corners() {
        let asset = rasterize_svg("close.svg", ICON, 1.0).expect("valid SVG");
        assert_eq!((asset.width(), asset.height()), (16, 16));
        assert!(
            asset
                .rgba_premultiplied()
                .chunks_exact(4)
                .any(|pixel| pixel[3] > 0)
        );
        assert_eq!(asset.rgba_premultiplied()[3], 0);
    }

    #[test]
    fn raster_size_changes_at_fractional_scale() {
        let asset = rasterize_svg("close.svg", ICON, 1.25).expect("valid SVG");
        assert_eq!((asset.width(), asset.height()), (20, 20));
    }
}
