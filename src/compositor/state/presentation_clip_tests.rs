use super::*;
use crate::presentation_animation::{
    AnimationCurve, AnimationTime, EasingCurve, PresentationClip, PresentationClipMutation,
    PresentationClipRect, PresentationGroupClip, PresentationSampleTimeSource,
    PresentationTransactionRequest,
};
use std::time::Duration;

fn clip_rect(x: f64, y: f64, width: f64, height: f64) -> PresentationClipRect {
    PresentationClipRect::new(x, y, width, height).expect("valid presentation Clip")
}

#[test]
fn immediate_canonical_clip_change_has_no_animation_track() {
    let mut state = CompositorState::new(None);
    let window_id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(window_id, 701))
        .expect("XDG window");
    let target = PresentationClip::Rect(clip_rect(8.0, 4.0, 60.0, 48.0));

    state
        .set_window_canonical_clip(window_id, target, None)
        .expect("immediate Clip update");

    assert_eq!(
        state
            .window(window_id)
            .expect("canonical DesktopWindow")
            .canonical_clip(),
        target
    );
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("WindowGroup scene node");
    assert!(!state.presentation_animator.has_clip_track(scene_node_id));
    assert_eq!(state.presentation_animator.active_count(), 0);
}

#[test]
fn logical_window_destruction_cancels_clip_but_keeps_physical_clip_evidence() {
    let mut state = CompositorState::new(None);
    state.presentation_animator.set_enabled(true);
    let window_id = state.allocate_window_id().expect("window id");
    let root_surface_id = 702;
    state
        .insert_desktop_window(DesktopWindow::new_xdg(window_id, root_surface_id))
        .expect("XDG window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("WindowGroup scene node");
    let target_clip = PresentationClip::Rect(clip_rect(12.0, 9.0, 40.0, 35.0));
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::clip(
            AnimationTime::from_nanos(0),
            vec![PresentationClipMutation::new(
                scene_node_id,
                PresentationClip::Rect(clip_rect(0.0, 0.0, 80.0, 70.0)),
                target_clip,
                None,
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("active Clip transaction");

    let output_id = state.ensure_native_output_id().expect("output id");
    let physical_clip = PresentationClip::Rect(clip_rect(4.0, 6.0, 65.0, 52.0));
    let mut sample = crate::presentation_animation::PresentationSceneSample::empty_for_output(
        output_id,
        AnimationTime::from_nanos(5_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
    );
    sample.clips.push(PresentationGroupClip::with_scene_node(
        scene_node_id,
        root_surface_id,
        physical_clip,
        physical_clip.rect(),
        None,
    ));
    state.publish_presented_presentation(9, &sample.frame_snapshot());

    assert!(state.remove_desktop_window(window_id).is_some());

    assert!(!state.presentation_animator.has_clip_track(scene_node_id));
    assert_eq!(state.presentation_animator.active_count(), 0);
    assert_eq!(state.presentation_animator.transaction_count(), 0);
    assert_eq!(
        state.presented_presentation_clip_for_scene_node(scene_node_id),
        physical_clip
    );
}
