use astrea_framework::{
    color::Rgba,
    component::ComponentState,
    geometry::{Rect, Size},
    layout::{BoxConstraints, Insets},
    theme::AstreaTheme,
    typography::FontWeight,
};

#[test]
fn theme_exposes_astrea_spacing_and_typography_tokens() {
    let theme = AstreaTheme::default_dark();

    assert_eq!(theme.spacing().md, 12.0);
    assert_eq!(theme.spacing().spotlight_margin, 14.0);
    assert_eq!(theme.typography().spotlight_search.family, "Inter");
    assert_eq!(theme.typography().spotlight_search.pixel_size, 22);
    assert_eq!(
        theme.typography().spotlight_search.weight,
        FontWeight::Light
    );
}

#[test]
fn insets_create_predictable_content_rects() {
    let rect = Rect::new(20.0, 10.0, 120.0, 80.0);
    let content = Insets::symmetric(12.0, 8.0).inset_rect(rect);

    assert_eq!(content, Rect::new(32.0, 18.0, 96.0, 64.0));
}

#[test]
fn box_constraints_clamp_component_sizes_without_negative_dimensions() {
    let constraints = BoxConstraints::new(Size::new(80.0, 36.0), Size::new(320.0, 120.0));

    assert_eq!(
        constraints.constrain(Size::new(24.0, 500.0)),
        Size::new(80.0, 120.0)
    );
}

#[test]
fn component_state_derives_opacity_from_enabled_and_pressed_state() {
    let disabled = ComponentState::default().disabled();
    let pressed = ComponentState::default().pressed(true);

    assert_eq!(
        disabled.content_alpha(Rgba::rgb(255, 255, 255)),
        Rgba::new(255, 255, 255, 102)
    );
    assert_eq!(
        pressed.surface_alpha(Rgba::rgb(0, 122, 255)),
        Rgba::new(0, 122, 255, 224)
    );
}
