use std::{fs, sync::OnceLock};

use ab_glyph::{Font, FontArc, PxScale, ScaleFont, point};
use exodus::shell::SpotlightFontWeight;

use super::canvas::{FrameSize, Rect, blend_pixel_checked};

static INTER_FONT: OnceLock<Option<FontArc>> = OnceLock::new();

const INTER_FONT_PATHS: &[&str] = &[
    "/usr/share/fonts/inter/InterVariable.ttf",
    "/usr/share/fonts/TTF/InterVariable.ttf",
    "/usr/local/share/fonts/InterVariable.ttf",
    "/home/agony/.local/share/fonts/InterVariable.ttf",
    "/usr/share/fonts/inter/Inter.ttc",
];

#[derive(Debug, Clone, Copy)]
pub(super) struct NativeTextStyle {
    pub(super) pixel_size: f32,
    pub(super) color: u32,
    pub(super) weight: SpotlightFontWeight,
}

pub(super) fn draw_native_text_in_rect(
    frame: &mut [u32],
    frame_size: FrameSize,
    rect: Rect,
    text: &str,
    style: NativeTextStyle,
) {
    let Some(font) = font_for_weight(style.weight) else {
        return;
    };
    let scale = PxScale::from(style.pixel_size);
    let scaled_font = font.as_scaled(scale);
    let text_height = scaled_font.ascent() - scaled_font.descent();
    let baseline = rect.y as f32 + (rect.height as f32 - text_height) / 2.0 + scaled_font.ascent();
    draw_native_text_at(
        frame,
        NativeTextDraw {
            frame_size,
            x: rect.x as f32,
            baseline,
            clip: rect,
            text,
            style,
            font,
        },
    );
}

pub(super) fn fit_native_text_to_width(
    text: &str,
    max_width: u32,
    style: NativeTextStyle,
) -> String {
    let Some(font) = font_for_weight(style.weight) else {
        return text.to_string();
    };
    let scale = PxScale::from(style.pixel_size);
    let scaled_font = font.as_scaled(scale);
    let mut fitted = String::new();
    let mut width = 0.0;
    let mut previous = None;
    for character in text.chars() {
        let glyph_id = scaled_font.glyph_id(character);
        let kern = previous
            .map(|previous| scaled_font.kern(previous, glyph_id))
            .unwrap_or(0.0);
        let next_width = width + kern + scaled_font.h_advance(glyph_id);
        if next_width > max_width as f32 {
            break;
        }
        fitted.push(character);
        width = next_width;
        previous = Some(glyph_id);
    }
    fitted
}

pub(super) fn measure_native_text_width(text: &str, style: NativeTextStyle) -> Option<f32> {
    let font = font_for_weight(style.weight)?;
    let scale = PxScale::from(style.pixel_size);
    let scaled_font = font.as_scaled(scale);
    let mut width = 0.0;
    let mut previous = None;
    for character in text.chars() {
        let glyph_id = scaled_font.glyph_id(character);
        if let Some(previous) = previous {
            width += scaled_font.kern(previous, glyph_id);
        }
        width += scaled_font.h_advance(glyph_id);
        previous = Some(glyph_id);
    }
    Some(width)
}

struct NativeTextDraw<'a> {
    frame_size: FrameSize,
    x: f32,
    baseline: f32,
    clip: Rect,
    text: &'a str,
    style: NativeTextStyle,
    font: &'a FontArc,
}

fn draw_native_text_at(frame: &mut [u32], request: NativeTextDraw<'_>) {
    let scale = PxScale::from(request.style.pixel_size);
    let scaled_font = request.font.as_scaled(scale);
    let mut x = request.x;
    let mut previous = None;
    for character in request.text.chars() {
        let glyph_id = scaled_font.glyph_id(character);
        if let Some(previous) = previous {
            x += scaled_font.kern(previous, glyph_id);
        }

        let glyph = glyph_id.with_scale_and_position(scale, point(x, request.baseline));
        if let Some(outlined) = scaled_font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|glyph_x, glyph_y, coverage| {
                let pixel_x = bounds.min.x as i32 + glyph_x as i32;
                let pixel_y = bounds.min.y as i32 + glyph_y as i32;
                if !request.clip.contains(pixel_x, pixel_y) {
                    return;
                }
                blend_pixel_checked(
                    frame,
                    request.frame_size.width,
                    request.frame_size.height,
                    pixel_x,
                    pixel_y,
                    premul_with_coverage(request.style.color, coverage),
                );
            });
        }

        x += scaled_font.h_advance(glyph_id);
        previous = Some(glyph_id);
    }
}

fn font_for_weight(_weight: SpotlightFontWeight) -> Option<&'static FontArc> {
    INTER_FONT
        .get_or_init(|| load_font(INTER_FONT_PATHS))
        .as_ref()
}

fn load_font(paths: &[&str]) -> Option<FontArc> {
    paths
        .iter()
        .filter_map(|path| fs::read(path).ok())
        .find_map(|bytes| FontArc::try_from_vec(bytes).ok())
}

fn premul_with_coverage(color: u32, coverage: f32) -> u32 {
    let factor = (coverage.clamp(0.0, 1.0) * 255.0).round() as u32;
    let scale_channel = |shift: u32| {
        let channel = (color >> shift) & 0xff;
        (channel * factor + 127) / 255
    };
    (scale_channel(24) << 24)
        | (scale_channel(16) << 16)
        | (scale_channel(8) << 8)
        | scale_channel(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::shell::canvas::premul_argb;

    #[test]
    fn astrea_spotlight_inter_weights_are_available() {
        assert!(font_for_weight(SpotlightFontWeight::Light).is_some());
        assert!(font_for_weight(SpotlightFontWeight::Regular).is_some());
        assert!(font_for_weight(SpotlightFontWeight::Medium).is_some());
    }

    #[test]
    fn native_text_draws_antialiased_inter_pixels() {
        let mut frame = vec![0; 240 * 80];
        draw_native_text_in_rect(
            &mut frame,
            FrameSize {
                width: 240,
                height: 80,
            },
            Rect::new(12, 12, 216, 40),
            "Spotlight Search",
            NativeTextStyle {
                pixel_size: 22.0,
                color: premul_argb(255, 255, 255, 255),
                weight: SpotlightFontWeight::Light,
            },
        );

        assert!(frame.iter().any(|pixel| pixel >> 24 != 0));
        assert!(
            frame
                .iter()
                .any(|pixel| (1..255).contains(&((pixel >> 24) & 0xff)))
        );
    }
}
