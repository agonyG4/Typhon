use astrea_framework::{
    geometry::{Rect, Size},
    theme::AstreaTheme,
};
use exodus::shell::{Spotlight, SpotlightFontWeight, SpotlightResult, SpotlightWeather, Topbar};

#[test]
fn topbar_layout_keeps_a_compact_astrea_bar_at_the_top_edge() {
    let theme = AstreaTheme::default_dark();
    let layout = Topbar::new("Oblivion One")
        .trailing_text("17:30")
        .layout(Size::new(1280.0, 720.0), &theme);

    assert_eq!(layout.bar_rect, Rect::new(12.0, 10.0, 1256.0, 34.0));
    assert_eq!(layout.leading_text, "Oblivion One");
    assert_eq!(layout.trailing_text.as_deref(), Some("17:30"));
    assert!(layout.material.blur.radius_px() > 0.0);
}

#[test]
fn spotlight_layout_matches_astrea_empty_panel_shape() {
    let theme = AstreaTheme::default_dark();
    let layout = Spotlight::new().layout(Size::new(1280.0, 720.0), &theme);

    assert_eq!(layout.panel_rect, Rect::new(340.0, 180.0, 600.0, 58.0));
    assert_eq!(layout.search_row_rect, Rect::new(354.0, 194.0, 572.0, 30.0));
    assert_eq!(layout.field_rect, Rect::new(398.0, 194.0, 452.0, 30.0));
    assert_eq!(layout.placeholder_text, "Spotlight Search");
    assert_eq!(layout.typography.font_family, "Inter");
    assert_eq!(layout.typography.search_pixel_size, 22);
    assert_eq!(layout.typography.search_weight, SpotlightFontWeight::Light);
    assert!(layout.material.background.alpha() < 255);
}

#[test]
fn spotlight_layout_expands_for_results_and_keeps_weather_inline() {
    let theme = AstreaTheme::default_dark();
    let layout = Spotlight::new()
        .query("fire")
        .weather(SpotlightWeather::ready("clear", 24))
        .results([
            SpotlightResult::new("Firefox", "firefox", true),
            SpotlightResult::new("Files", "nautilus", false),
        ])
        .layout(Size::new(1280.0, 720.0), &theme);

    assert_eq!(layout.panel_rect, Rect::new(340.0, 180.0, 600.0, 179.0));
    assert_eq!(layout.weather_rect, Rect::new(858.0, 194.0, 68.0, 30.0));
    assert_eq!(
        layout.divider_rect,
        Some(Rect::new(354.0, 236.0, 572.0, 1.0))
    );
    assert_eq!(layout.result_rows.len(), 2);
    assert_eq!(
        layout.result_rows[0].rect,
        Rect::new(354.0, 245.0, 572.0, 50.0)
    );
    assert_eq!(
        layout.result_rows[0].icon_rect,
        Rect::new(366.0, 250.0, 40.0, 40.0)
    );
    assert_eq!(
        layout.result_rows[0].text_rect,
        Rect::new(421.0, 245.0, 493.0, 50.0)
    );
    assert!(layout.result_rows[0].selected);
    assert_eq!(layout.typography.result_pixel_size, 17);
    assert_eq!(layout.typography.weather_pixel_size, 18);
    assert_eq!(
        layout.typography.weather_weight,
        SpotlightFontWeight::Medium
    );
}
