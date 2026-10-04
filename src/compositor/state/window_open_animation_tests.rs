use super::window_open_animation::window_open_animation_plan;
use super::*;
use crate::animation_control::{
    AnimationConfiguration, AnimationConfigurationStore, AnimationControlState, AnimationEffect,
    AnimationPreset, AnimationRuntimeCapabilities,
};
use crate::presentation_animation::{
    AnimationCurve, AnimationTime, EasingCurve, PresentationOpacity, PresentationRect,
    PresentationTransactionRequest,
};
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static ANIMATION_TEST_CONFIG_ID: AtomicU64 = AtomicU64::new(1);

fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
    PresentationRect::new(x, y, width, height).expect("valid presentation rectangle")
}

fn mapped_test_window() -> (CompositorState, WindowId, u32) {
    let mut state = CompositorState::new(None);
    let root_surface_id = 91;
    let window_id = state.allocate_window_id().expect("test window ID");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(window_id, root_surface_id))
        .expect("test XDG window");
    let placement = state.surface_placement(root_surface_id);
    state.append_renderable_surface(super::desktop_window_tests::x11_shm_surface(
        root_surface_id,
        500,
        400,
        placement,
    ));
    state
        .surface_presentation_generations
        .insert(root_surface_id, 1);
    state.rebuild_active_scene_view();
    state.presentation_animator.set_enabled(true);
    (state, window_id, root_surface_id)
}

pub(super) fn set_window_open_preset(state: &mut CompositorState, preset: AnimationPreset) {
    set_window_open_preset_with_maximized_policy(state, preset, true);
}

pub(super) fn set_window_open_preset_with_maximized_policy(
    state: &mut CompositorState,
    preset: AnimationPreset,
    animate_maximized_window_open: bool,
) {
    let test_temp_dir = std::env::current_exe()
        .expect("Cargo test executable")
        .parent()
        .expect("Cargo test executable directory")
        .to_path_buf();
    let directory = test_temp_dir.join(format!(
        "animation-config-{}-{}",
        std::process::id(),
        ANIMATION_TEST_CONFIG_ID.fetch_add(1, Ordering::Relaxed),
    ));
    std::fs::create_dir(&directory).expect("animation test configuration directory");
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .expect("private animation test configuration directory");
    state.animation_control = AnimationControlState::from_store(
        AnimationConfigurationStore::new(directory.clone())
            .expect("animation test configuration store"),
    );
    let configuration = AnimationConfiguration {
        preset,
        animate_maximized_window_open,
        ..AnimationConfiguration::default()
    };
    state
        .animation_control
        .set_configuration(configuration, AnimationRuntimeCapabilities::default())
        .expect("animation test configuration");
    std::fs::remove_dir_all(directory).expect("remove animation test configuration");
}

#[test]
fn scale_plan_is_centered_at_ninety_four_percent_without_an_opacity_transition() {
    let target = rect(100.0, 200.0, 500.0, 400.0);
    let opacity = PresentationOpacity::new(0.37).expect("canonical opacity");
    let plan = window_open_animation_plan(AnimationEffect::WindowScale, target, opacity, 1.0)
        .expect("scale plan");

    assert_eq!(plan.geometry_start, rect(115.0, 212.0, 470.0, 376.0));
    assert_eq!(plan.geometry_target, target);
    assert!(plan.opacity.is_none());
}

#[test]
fn glide_plan_keeps_size_and_starts_twenty_four_logical_pixels_below_target() {
    let target = rect(100.0, 200.0, 500.0, 400.0);
    let opacity = PresentationOpacity::new(0.37).expect("canonical opacity");
    let plan = window_open_animation_plan(AnimationEffect::WindowGlide, target, opacity, 1.0)
        .expect("glide plan");

    assert_eq!(plan.geometry_start, rect(100.0, 224.0, 500.0, 400.0));
    assert_eq!(plan.geometry_target, target);
    let opacity_transition = plan.opacity.expect("WindowGlide opacity transition");
    assert_eq!(opacity_transition.start, PresentationOpacity::TRANSPARENT);
    assert_eq!(opacity_transition.target, opacity);
}

#[test]
fn open_plan_uses_ease_out_cubic_and_existing_speed_scaling() {
    let target = rect(10.0, 20.0, 100.0, 80.0);
    let opacity = PresentationOpacity::new(0.6).expect("canonical opacity");
    for (speed, expected_duration) in [(1.0, 180), (2.0, 90), (0.5, 360)] {
        let scale_plan =
            window_open_animation_plan(AnimationEffect::WindowScale, target, opacity, speed)
                .expect("scale plan");
        assert!(matches!(
            scale_plan.geometry_curve,
            AnimationCurve::Easing {
                duration,
                curve: EasingCurve::EaseOutCubic,
            } if duration == Duration::from_millis(expected_duration)
        ));

        let glide_plan =
            window_open_animation_plan(AnimationEffect::WindowGlide, target, opacity, speed)
                .expect("glide plan");
        assert!(matches!(
            glide_plan.geometry_curve,
            AnimationCurve::Easing {
                duration,
                curve: EasingCurve::EaseOutCubic,
            } if duration == Duration::from_millis(expected_duration)
        ));
        assert!(matches!(
            glide_plan
                .opacity
                .expect("WindowGlide opacity transition")
                .curve,
            AnimationCurve::Easing {
                duration,
                curve: EasingCurve::EaseOutCubic,
            } if duration == Duration::from_millis(expected_duration)
        ));
    }
}

#[test]
fn window_scale_commits_geometry_only_and_preserves_canonical_translucency() {
    let (mut state, window_id, root_surface_id) = mapped_test_window();
    set_window_open_preset_with_maximized_policy(&mut state, AnimationPreset::Astrea, false);
    let target_opacity = PresentationOpacity::new(0.42).expect("canonical opacity");
    state
        .window_mut(window_id)
        .expect("test window")
        .set_canonical_opacity(target_opacity);
    let canonical_geometry = state
        .current_root_window_geometry(root_surface_id)
        .expect("canonical geometry");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group node");
    state
        .ensure_native_output_id()
        .expect("test output identity");

    assert!(state.maybe_begin_window_open_animation(root_surface_id));
    assert!(state.window_open_presentation_active(root_surface_id));
    assert_eq!(
        state.window(window_id).expect("test window").state.mode(),
        ToplevelMode::Normal
    );

    assert!(
        state
            .presentation_animator
            .has_geometry_track(scene_node_id)
    );
    assert!(!state.presentation_animator.has_opacity_track(scene_node_id));
    assert_eq!(
        state
            .presentation_animator
            .opacity_track_transaction(scene_node_id),
        None
    );
    let started_at = state
        .presentation_animator
        .track_started_at_for_scene_node(scene_node_id)
        .expect("WindowScale Geometry start time");
    assert_eq!(
        state
            .presentation_scene_sample_at(started_at)
            .opacity_for_scene_node(scene_node_id),
        target_opacity
    );
    assert_eq!(
        state
            .presentation_scene_sample_at(AnimationTime::from_nanos(u64::MAX))
            .opacity_for_scene_node(scene_node_id),
        target_opacity
    );
    assert_eq!(
        state.current_root_window_geometry(root_surface_id),
        Some(canonical_geometry)
    );
    assert_eq!(
        state
            .window(window_id)
            .expect("test window")
            .canonical_opacity(),
        target_opacity
    );
    assert_eq!(state.presentation_animator.transaction_count(), 1);
}

#[test]
fn window_scale_preserves_existing_geometry_owner_without_adding_a_transaction() {
    let (mut state, window_id, root_surface_id) = mapped_test_window();
    set_window_open_preset(&mut state, AnimationPreset::Astrea);
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group node");
    let target = state
        .current_presentation_rect_for_root(root_surface_id)
        .expect("canonical presentation rectangle");
    let now = AnimationTime::monotonic_now().expect("monotonic time");
    let geometry_start = rect(
        target.x() - 40.0,
        target.y(),
        target.width(),
        target.height(),
    );
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::geometry(
            now,
            vec![
                crate::presentation_animation::PresentationGeometryMutation::new(
                    scene_node_id,
                    geometry_start,
                    target,
                    AnimationCurve::easing(Duration::from_millis(400), EasingCurve::Linear),
                ),
            ],
        ))
        .expect("existing geometry transition");
    let geometry_transaction = state
        .presentation_animator
        .track_transaction(scene_node_id)
        .expect("geometry transaction");

    assert!(!state.maybe_begin_window_open_animation(root_surface_id));

    assert_eq!(
        state.presentation_animator.track_transaction(scene_node_id),
        Some(geometry_transaction)
    );
    assert!(!state.presentation_animator.has_opacity_track(scene_node_id));
    assert_eq!(
        state
            .presentation_animator
            .opacity_track_transaction(scene_node_id),
        None
    );
    assert_eq!(state.presentation_animator.transaction_count(), 1);
    assert_eq!(
        state
            .presentation_animator
            .sample_at_transition_start_for_scene_node(scene_node_id)
            .expect("unrelated Geometry owner sample")
            .rect,
        geometry_start
    );
}

#[test]
fn window_glide_still_commits_geometry_and_opacity_together() {
    let (mut state, window_id, root_surface_id) = mapped_test_window();
    set_window_open_preset_with_maximized_policy(&mut state, AnimationPreset::Macos, false);
    let canonical_opacity = PresentationOpacity::new(0.42).expect("canonical opacity");
    state
        .window_mut(window_id)
        .expect("test window")
        .set_canonical_opacity(canonical_opacity);
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group node");
    let target = state
        .current_presentation_rect_for_root(root_surface_id)
        .expect("canonical presentation rectangle");

    assert!(state.maybe_begin_window_open_animation(root_surface_id));

    let geometry_transaction = state
        .presentation_animator
        .track_transaction(scene_node_id)
        .expect("WindowGlide Geometry transaction");
    assert!(state.presentation_animator.has_opacity_track(scene_node_id));
    assert_eq!(
        state
            .presentation_animator
            .opacity_track_transaction(scene_node_id),
        Some(geometry_transaction)
    );
    assert_eq!(
        state
            .presentation_animator
            .sample_at_transition_start_for_scene_node(scene_node_id)
            .expect("WindowGlide Geometry start sample")
            .rect,
        rect(
            target.x(),
            target.y() + 24.0,
            target.width(),
            target.height()
        )
    );
    assert_eq!(
        state
            .presentation_animator
            .sample_opacity_for_scene_node(scene_node_id, AnimationTime::from_nanos(0))
            .expect("WindowGlide Opacity start sample")
            .0,
        PresentationOpacity::TRANSPARENT
    );
    assert_eq!(
        state
            .presentation_animator
            .sample_opacity_for_scene_node(scene_node_id, AnimationTime::from_nanos(u64::MAX),)
            .expect("WindowGlide Opacity final sample")
            .0,
        canonical_opacity
    );
    assert_eq!(state.presentation_animator.transaction_count(), 1);
}

#[test]
fn window_glide_with_existing_geometry_owner_adds_only_its_opacity_track() {
    let (mut state, window_id, root_surface_id) = mapped_test_window();
    set_window_open_preset(&mut state, AnimationPreset::Macos);
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group node");
    let target = state
        .current_presentation_rect_for_root(root_surface_id)
        .expect("canonical presentation rectangle");
    let geometry_start = rect(
        target.x() - 40.0,
        target.y(),
        target.width(),
        target.height(),
    );
    let now = AnimationTime::monotonic_now().expect("monotonic time");
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::geometry(
            now,
            vec![
                crate::presentation_animation::PresentationGeometryMutation::new(
                    scene_node_id,
                    geometry_start,
                    target,
                    AnimationCurve::easing(Duration::from_millis(400), EasingCurve::Linear),
                ),
            ],
        ))
        .expect("existing geometry transition");
    let geometry_transaction = state
        .presentation_animator
        .track_transaction(scene_node_id)
        .expect("existing Geometry transaction");

    assert!(state.maybe_begin_window_open_animation(root_surface_id));
    assert!(state.window_open_presentation_active(root_surface_id));

    assert_eq!(
        state.presentation_animator.track_transaction(scene_node_id),
        Some(geometry_transaction)
    );
    assert!(state.presentation_animator.has_opacity_track(scene_node_id));
    let opacity_transaction = state
        .presentation_animator
        .opacity_track_transaction(scene_node_id)
        .expect("WindowGlide Opacity transaction");
    assert_ne!(opacity_transaction, geometry_transaction);
    assert_eq!(state.presentation_animator.transaction_count(), 2);
    assert_eq!(
        state
            .presentation_animator
            .sample_at_transition_start_for_scene_node(scene_node_id)
            .expect("unrelated Geometry owner sample")
            .rect,
        geometry_start
    );
    assert_eq!(
        state
            .presentation_animator
            .sample_opacity_for_scene_node(scene_node_id, AnimationTime::from_nanos(0))
            .expect("WindowGlide opacity-only fallback sample")
            .0,
        PresentationOpacity::TRANSPARENT
    );
}

#[test]
fn minimize_or_disabled_presentation_fails_open_without_open_tracks() {
    let (mut minimized, window_id, root_surface_id) = mapped_test_window();
    minimized
        .window_mut(window_id)
        .expect("test window")
        .state
        .mark_minimized_without_surfaces();
    assert!(!minimized.maybe_begin_window_open_animation(root_surface_id));

    let (mut disabled, window_id, root_surface_id) = mapped_test_window();
    let scene_node_id = disabled
        .scene_node_id_for_window_group(window_id)
        .expect("window group node");
    disabled.presentation_animator.set_enabled(false);
    assert!(!disabled.maybe_begin_window_open_animation(root_surface_id));
    assert!(!disabled.presentation_animator.has_track(scene_node_id));
}

#[test]
fn removing_a_window_during_window_scale_cancels_geometry_ownership() {
    let (mut state, window_id, root_surface_id) = mapped_test_window();
    set_window_open_preset(&mut state, AnimationPreset::Astrea);
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group node");
    assert!(state.maybe_begin_window_open_animation(root_surface_id));
    assert!(
        state
            .presentation_animator
            .has_geometry_track(scene_node_id)
    );
    assert!(!state.presentation_animator.has_opacity_track(scene_node_id));
    assert_eq!(
        state
            .presentation_animator
            .opacity_track_transaction(scene_node_id),
        None
    );

    state
        .remove_desktop_window(window_id)
        .expect("remove window");

    assert!(
        !state
            .window_open_presentation_ownership
            .contains_key(&root_surface_id)
    );
    assert!(!state.presentation_animator.has_track(scene_node_id));
    assert_eq!(state.presentation_animator.transaction_count(), 0);
}
