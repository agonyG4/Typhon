use std::sync::Arc;

use ab_glyph::{Font, FontArc, ScaleFont, point};

use super::{raster::DecorationRasterAsset, theme::ThemeTitleAlignment, types::DecorationRect};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RasterizedTitle {
    pub(crate) text: String,
    pub(crate) logical_width: u32,
    pub(crate) asset: DecorationRasterAsset,
}

pub(crate) fn rasterize_title(
    font_bytes: &[u8],
    title: &str,
    title_rect: DecorationRect,
    color: [u8; 4],
    font_size: u32,
    alignment: ThemeTitleAlignment,
    output_scale: f64,
) -> Result<RasterizedTitle, String> {
    let font = FontArc::try_from_vec(font_bytes.to_vec())
        .map_err(|error| format!("invalid selected title font: {error}"))?;
    let scale = output_scale.max(0.01);
    let font_px = (font_size as f64 * scale) as f32;
    let scaled = font.as_scaled(font_px);
    let max_width = (f64::from(title_rect.width) * scale).floor().max(1.0) as f32;
    let text = fit_title(&scaled, title, max_width);
    let measured_width = measured_width(&scaled, &text).ceil().max(1.0);
    let width = measured_width as u32;
    let height = (f64::from(title_rect.height) * scale).ceil().max(1.0) as u32;
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    let baseline = (height as f32 - scaled.height()).max(0.0) / 2.0 + scaled.ascent();
    let mut cursor = 0.0f32;
    let mut previous = None;
    for character in text.chars() {
        let glyph_id = scaled.glyph_id(character);
        if let Some(previous) = previous {
            cursor += scaled.kern(previous, glyph_id);
        }
        let glyph = glyph_id.with_scale_and_position(font_px, point(cursor, baseline));
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|x, y, coverage| {
                let target_x = bounds.min.x.floor() as i32 + x as i32;
                let target_y = bounds.min.y.floor() as i32 + y as i32;
                if target_x < 0
                    || target_y < 0
                    || target_x >= width as i32
                    || target_y >= height as i32
                {
                    return;
                }
                let alpha = (f32::from(color[3]) * coverage.clamp(0.0, 1.0)).round() as u8;
                let index = (target_y as usize * width as usize + target_x as usize) * 4;
                pixels[index] = premultiply(color[0], alpha);
                pixels[index + 1] = premultiply(color[1], alpha);
                pixels[index + 2] = premultiply(color[2], alpha);
                pixels[index + 3] = alpha;
            });
        }
        cursor += scaled.h_advance(glyph_id);
        previous = Some(glyph_id);
    }

    let logical_width = ((f64::from(width) / scale).ceil() as u32).max(1);
    let alignment_bias = match alignment {
        ThemeTitleAlignment::Left => 0,
        ThemeTitleAlignment::Center => 1,
        ThemeTitleAlignment::Right => 2,
    };
    let asset_id = title_asset_id(
        &text,
        color,
        font_size,
        output_scale,
        alignment_bias,
        &pixels,
    );
    Ok(RasterizedTitle {
        text,
        logical_width,
        asset: DecorationRasterAsset::from_pixels(
            asset_id,
            width,
            height,
            Arc::<[u8]>::from(pixels),
        ),
    })
}

fn fit_title<F: Font>(font: &ab_glyph::PxScaleFont<F>, title: &str, max_width: f32) -> String {
    if measured_width(font, title) <= max_width {
        return title.to_owned();
    }
    let ellipsis = '…';
    let ellipsis_width = measured_width(font, &ellipsis.to_string());
    if ellipsis_width > max_width {
        return ellipsis.to_string();
    }
    let mut result = String::new();
    for character in title.chars() {
        let candidate = format!("{result}{character}{ellipsis}");
        if measured_width(font, &candidate) > max_width {
            break;
        }
        result.push(character);
    }
    result.push(ellipsis);
    result
}

fn measured_width<F: Font>(font: &ab_glyph::PxScaleFont<F>, text: &str) -> f32 {
    let mut width = 0.0;
    let mut previous = None;
    for character in text.chars() {
        let glyph = font.glyph_id(character);
        if let Some(previous) = previous {
            width += font.kern(previous, glyph);
        }
        width += font.h_advance(glyph);
        previous = Some(glyph);
    }
    width
}

fn premultiply(component: u8, alpha: u8) -> u8 {
    (u16::from(component) * u16::from(alpha) / 255) as u8
}

fn title_asset_id(
    text: &str,
    color: [u8; 4],
    font_size: u32,
    output_scale: f64,
    alignment: u8,
    pixels: &[u8],
) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in text
        .as_bytes()
        .iter()
        .copied()
        .chain(color)
        .chain(font_size.to_le_bytes())
        .chain(output_scale.to_bits().to_le_bytes())
        .chain([alignment])
        .chain(pixels.iter().copied())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::rasterize_title;
    use crate::compositor::decoration::{theme::ThemeTitleAlignment, types::DecorationRect};

    #[test]
    fn measured_title_uses_unicode_and_ellipsis() {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let id = db
            .query(&fontdb::Query {
                families: &[fontdb::Family::Name("DejaVu Sans")],
                ..fontdb::Query::default()
            })
            .expect("system sans font");
        let bytes = db
            .with_face_data(id, |data, _| data.to_vec())
            .expect("font bytes");
        let title = rasterize_title(
            &bytes,
            "東京 window title that is deliberately long",
            DecorationRect::new(0, 0, 110, 26),
            [255, 255, 255, 255],
            13,
            ThemeTitleAlignment::Center,
            1.0,
        )
        .expect("title raster");
        assert!(title.text.ends_with('…'));
        assert!(
            title
                .asset
                .rgba_premultiplied()
                .iter()
                .any(|byte| *byte != 0)
        );
    }
}
