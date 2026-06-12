use astrea_framework::{component::ComponentState, geometry::Rect, theme::AstreaTheme};
use exodus::components::{Button, ButtonVariant, ListRow, Panel, Slider, TextField, ToggleSwitch};

#[test]
fn panel_layout_exposes_padded_content_rect_and_glass_material() {
    let theme = AstreaTheme::default_dark();
    let layout = Panel::new(Rect::new(10.0, 20.0, 300.0, 180.0))
        .padding(theme.spacing().xl)
        .layout(&theme);

    assert_eq!(layout.frame_rect, Rect::new(10.0, 20.0, 300.0, 180.0));
    assert_eq!(layout.content_rect, Rect::new(28.0, 38.0, 264.0, 144.0));
    assert!(layout.material.blur.radius_px() >= 20.0);
}

#[test]
fn button_layout_promotes_small_rect_to_astrea_hit_target() {
    let theme = AstreaTheme::default_dark();
    let layout = Button::new("Continue")
        .variant(ButtonVariant::Primary)
        .layout(Rect::new(40.0, 30.0, 40.0, 20.0), &theme);

    assert_eq!(layout.frame_rect, Rect::new(40.0, 30.0, 96.0, 36.0));
    assert_eq!(layout.content_rect, Rect::new(54.0, 38.0, 68.0, 20.0));
    assert_eq!(layout.label, "Continue");
    assert_eq!(layout.text_style.family, "Inter");
}

#[test]
fn text_field_layout_keeps_placeholder_and_cursor_inside_control_padding() {
    let theme = AstreaTheme::default_dark();
    let layout = TextField::new("Search")
        .value("fire")
        .state(ComponentState::default().focused(true))
        .layout(Rect::new(0.0, 0.0, 260.0, 30.0), &theme);

    assert_eq!(layout.frame_rect, Rect::new(0.0, 0.0, 260.0, 36.0));
    assert_eq!(layout.text_rect, Rect::new(14.0, 8.0, 232.0, 20.0));
    assert_eq!(layout.cursor_rect, Some(Rect::new(48.0, 9.0, 1.0, 18.0)));
    assert_eq!(layout.placeholder, "Search");
}

#[test]
fn toggle_switch_layout_moves_thumb_between_off_and_on_positions() {
    let theme = AstreaTheme::default_dark();
    let off = ToggleSwitch::new(false).layout(Rect::new(100.0, 40.0, 0.0, 0.0), &theme);
    let on = ToggleSwitch::new(true).layout(Rect::new(100.0, 40.0, 0.0, 0.0), &theme);

    assert_eq!(off.track_rect, Rect::new(100.0, 40.0, 46.0, 28.0));
    assert_eq!(off.thumb_rect, Rect::new(103.0, 43.0, 22.0, 22.0));
    assert_eq!(on.thumb_rect, Rect::new(121.0, 43.0, 22.0, 22.0));
}

#[test]
fn slider_layout_clamps_value_and_tracks_fill_width() {
    let theme = AstreaTheme::default_dark();
    let layout = Slider::new(1.4).layout(Rect::new(20.0, 20.0, 200.0, 28.0), &theme);

    assert_eq!(layout.value, 1.0);
    assert_eq!(layout.track_rect, Rect::new(20.0, 32.0, 200.0, 4.0));
    assert_eq!(layout.fill_rect, Rect::new(20.0, 32.0, 200.0, 4.0));
    assert_eq!(layout.thumb_rect, Rect::new(208.0, 24.0, 20.0, 20.0));
}

#[test]
fn list_row_layout_matches_astrea_spotlight_row_metrics() {
    let theme = AstreaTheme::default_dark();
    let layout = ListRow::new("Firefox")
        .subtitle("firefox")
        .state(ComponentState::default().selected(true))
        .layout(Rect::new(354.0, 245.0, 572.0, 50.0), &theme);

    assert_eq!(layout.row_rect, Rect::new(354.0, 245.0, 572.0, 50.0));
    assert_eq!(layout.icon_rect, Rect::new(366.0, 250.0, 40.0, 40.0));
    assert_eq!(layout.text_rect, Rect::new(421.0, 245.0, 493.0, 50.0));
    assert!(layout.selected);
}
