use astrea_framework::{geometry::Size, theme::AstreaTheme};
use exodus::shell::{Topbar, TopbarLayout};

use super::{
    canvas::{FrameSize, Rect, draw_rounded_rect, premul_argb},
    font_text::{
        NativeTextStyle, draw_native_text_in_rect, fit_native_text_to_width,
        measure_native_text_width,
    },
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellTopbarModel {
    visible: bool,
    title: String,
    trailing_text: String,
}

impl ShellTopbarModel {
    pub fn visible(title: impl Into<String>) -> Self {
        Self {
            visible: true,
            title: title.into(),
            trailing_text: String::new(),
        }
    }

    pub fn with_trailing_text(mut self, trailing_text: impl Into<String>) -> Self {
        self.trailing_text = trailing_text.into();
        self
    }

    pub const fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn trailing_text(&self) -> &str {
        &self.trailing_text
    }
}

pub(super) fn topbar_bounds(width: u32, height: u32, topbar: &ShellTopbarModel) -> Option<Rect> {
    topbar_rect(width, height, topbar)
}

pub(super) fn draw_topbar_at(
    frame: &mut [u32],
    width: u32,
    height: u32,
    output_width: u32,
    output_height: u32,
    origin: (i32, i32),
    topbar: &ShellTopbarModel,
) {
    let Some(output_rect) = topbar_rect(output_width, output_height, topbar) else {
        return;
    };
    let rect = output_rect.translated(-origin.0, -origin.1);

    let theme = AstreaTheme::default_dark();
    let layout = topbar_layout(output_width, output_height, topbar, &theme);

    draw_rounded_rect(frame, width, height, rect, 14, premul_argb(210, 18, 22, 30));

    let label_tokens = theme.typography().label;
    let leading_style = NativeTextStyle {
        pixel_size: label_tokens.pixel_size as f32,
        color: premul_argb(245, 236, 240, 248),
        weight: label_tokens.weight,
    };
    let trailing_style = NativeTextStyle {
        pixel_size: label_tokens.pixel_size as f32,
        color: premul_argb(220, 190, 198, 212),
        weight: label_tokens.weight,
    };
    let frame_size = FrameSize { width, height };
    let trailing_width = layout
        .trailing_text
        .as_deref()
        .filter(|text| !text.is_empty())
        .map(|text| fitted_text_width(text, 220, trailing_style))
        .unwrap_or(0);
    let trailing_reserved_width = if trailing_width == 0 {
        0
    } else {
        trailing_width.saturating_add(16)
    };
    let leading_max_width = rect
        .width
        .saturating_sub(32)
        .saturating_sub(trailing_reserved_width);
    let leading_text =
        fit_native_text_to_width(&layout.leading_text, leading_max_width, leading_style);
    draw_native_text_in_rect(
        frame,
        frame_size,
        Rect::new(rect.x + 16, rect.y, leading_max_width, rect.height),
        &leading_text,
        leading_style,
    );

    if let Some(trailing_text) = layout.trailing_text.filter(|text| !text.is_empty()) {
        let fitted_text = fit_native_text_to_width(&trailing_text, 220, trailing_style);
        let width = fitted_text_width(&fitted_text, 220, trailing_style);
        draw_native_text_in_rect(
            frame,
            frame_size,
            Rect::new(
                rect.x + rect.width as i32 - width as i32 - 16,
                rect.y,
                width,
                rect.height,
            ),
            &fitted_text,
            trailing_style,
        );
    }
}

fn fitted_text_width(text: &str, max_width: u32, style: NativeTextStyle) -> u32 {
    if max_width == 0 {
        return 0;
    }

    measure_native_text_width(text, style)
        .map(|width| width.ceil() as u32)
        .unwrap_or(max_width)
        .clamp(1, max_width)
}

fn topbar_rect(width: u32, height: u32, topbar: &ShellTopbarModel) -> Option<Rect> {
    if !topbar.is_visible() || width < 220 || height < 64 {
        return None;
    }

    let theme = AstreaTheme::default_dark();
    let layout = topbar_layout(width, height, topbar, &theme);
    Some(Rect::new(
        layout.bar_rect.x.round() as i32,
        layout.bar_rect.y.round() as i32,
        layout.bar_rect.width.round() as u32,
        layout.bar_rect.height.round() as u32,
    ))
}

fn topbar_layout(
    width: u32,
    height: u32,
    topbar: &ShellTopbarModel,
    theme: &AstreaTheme,
) -> TopbarLayout {
    Topbar::new(topbar.title())
        .trailing_text(topbar.trailing_text())
        .layout(Size::new(width as f32, height as f32), theme)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topbar_model_can_carry_title_and_trailing_text() {
        let model = ShellTopbarModel::visible("Oblivion One").with_trailing_text("17:30");

        assert!(model.is_visible());
        assert_eq!(model.title(), "Oblivion One");
        assert_eq!(model.trailing_text(), "17:30");
    }
}
