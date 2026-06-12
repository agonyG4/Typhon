use astrea_framework::{
    color::Rgba,
    geometry::{CornerRadii, Rect, Size},
    material::MaterialRole,
    motion::{DurationMs, MotionSpeed},
    performance::FrameBudget,
    render::{DrawCommand, RenderPlan},
    theme::AstreaTheme,
};

#[test]
fn default_theme_exposes_glass_material_tokens_for_exodus() {
    let theme = AstreaTheme::default_dark();
    let window = theme.material(MaterialRole::Window);
    let button = theme.material(MaterialRole::Control);

    assert_eq!(theme.name(), "Astrea Dark");
    assert!(window.blur.radius_px() >= 18.0);
    assert!(window.background.alpha() < 255);
    assert!(button.background.alpha() > window.background.alpha());
    assert!(theme.radius().window.max_radius() > theme.radius().control.max_radius());
}

#[test]
fn motion_speed_scales_duration_without_losing_frame_safe_minimums() {
    let fast = MotionSpeed::from_multiplier(1.5).unwrap();
    let slow = MotionSpeed::from_multiplier(0.5).unwrap();
    let disabled = MotionSpeed::instant();

    assert_eq!(
        fast.scale_duration(DurationMs::new(240)),
        DurationMs::new(160)
    );
    assert_eq!(
        slow.scale_duration(DurationMs::new(240)),
        DurationMs::new(480)
    );
    assert_eq!(
        disabled.scale_duration(DurationMs::new(240)),
        DurationMs::ZERO
    );
}

#[test]
fn frame_budget_tracks_refresh_rate_and_effect_costs() {
    let budget = FrameBudget::from_refresh_rate_hz(165).unwrap();

    assert_eq!(budget.frame_time_micros(), 6060);
    assert!(budget.allows_effect_cost_micros(1200));
    assert!(!budget.allows_effect_cost_micros(7000));
}

#[test]
fn render_plan_emits_backdrop_blur_before_material_fill() {
    let theme = AstreaTheme::default_dark();
    let rect = Rect::new(20.0, 30.0, 320.0, 220.0);
    let mut plan = RenderPlan::new(Size::new(800.0, 600.0));

    plan.push_material_rect(rect, CornerRadii::all(24.0), MaterialRole::Window, &theme);

    assert_eq!(
        plan.commands(),
        &[
            DrawCommand::BackdropBlur {
                rect,
                radius_px: 24.0,
            },
            DrawCommand::RoundedRect {
                rect,
                radii: CornerRadii::all(24.0),
                color: Rgba::new(18, 20, 28, 214),
            },
        ],
    );
}
