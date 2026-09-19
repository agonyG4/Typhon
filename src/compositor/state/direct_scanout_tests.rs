use super::desktop_window_tests::{
    install_x11_scanout_surface, x11_output_snapshot, x11_scanout_surface,
};
use super::*;
use crate::presentation_animation::{
    AnimationCurve, AnimationTime, EasingCurve, PresentationGeometryMutation, PresentationOpacity,
    PresentationOpacityMutation, PresentationRect, PresentationTransactionRequest,
};
use crate::render_backend::buffer::DrmFormat;
use crate::xwayland::XwaylandGeneration;
use std::num::NonZeroU64;
use std::time::Duration;

fn install_off_output_xdg_window(state: &mut CompositorState, root_surface_id: u32) -> SceneNodeId {
    let window_id = state.allocate_window_id().expect("off-output window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(window_id, root_surface_id))
        .expect("off-output window");
    state.append_renderable_surface(super::desktop_window_tests::x11_shm_surface(
        root_surface_id,
        16,
        16,
        SurfacePlacement::absolute_root_at(2_000, 0),
    ));
    state
        .surface_presentation_generations
        .insert(root_surface_id, 1);
    state.rebuild_active_scene_view();
    state
        .scene_node_id_for_window_group(window_id)
        .expect("off-output WindowGroup scene node")
}

#[test]
fn candidate_opacity_track_blocks_direct_scanout() {
    let mut state = CompositorState::new(None);
    let (width, height) = (state.output_size.width, state.output_size.height);
    let generation = XwaylandGeneration::new(NonZeroU64::new(35).expect("generation"));
    install_x11_scanout_surface(
        &mut state,
        x11_scanout_surface(
            351,
            width,
            height,
            SurfacePlacement::absolute_root_at(0, 0),
            DrmFormat::Xrgb8888,
        ),
        x11_output_snapshot(generation, 351, 351),
    );
    let window_id = state.window_id_for_surface(351).expect("visible window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("visible WindowGroup scene node");
    state.presentation_animator.set_enabled(true);
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::opacity(
            AnimationTime::from_nanos(0),
            vec![PresentationOpacityMutation::new(
                scene_node_id,
                PresentationOpacity::OPAQUE,
                PresentationOpacity::new(0.5).expect("half opacity"),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("visible opacity transaction");

    assert!(
        state
            .direct_scanout_scene_blockers()
            .reasons()
            .contains(&DirectScanoutSceneRejection::PresentationOpacity)
    );
}

#[test]
fn unrelated_off_output_opacity_track_does_not_block_direct_scanout() {
    let mut state = CompositorState::new(None);
    let (width, height) = (state.output_size.width, state.output_size.height);
    let generation = XwaylandGeneration::new(NonZeroU64::new(352).expect("generation"));
    install_x11_scanout_surface(
        &mut state,
        x11_scanout_surface(
            352,
            width,
            height,
            SurfacePlacement::absolute_root_at(0, 0),
            DrmFormat::Xrgb8888,
        ),
        x11_output_snapshot(generation, 352, 352),
    );
    let unrelated_node = install_off_output_xdg_window(&mut state, 353);
    state.presentation_animator.set_enabled(true);
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::opacity(
            AnimationTime::from_nanos(0),
            vec![PresentationOpacityMutation::new(
                unrelated_node,
                PresentationOpacity::OPAQUE,
                PresentationOpacity::new(0.5).expect("half opacity"),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("off-output opacity transaction");

    let blockers = state.direct_scanout_scene_blockers();
    assert!(
        !blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::PresentationOpacity)
    );
    assert!(state.direct_scanout_scene_candidate().is_ok());
}

#[test]
fn candidate_geometry_track_blocks_direct_scanout() {
    let mut state = CompositorState::new(None);
    let (width, height) = (state.output_size.width, state.output_size.height);
    let generation = XwaylandGeneration::new(NonZeroU64::new(354).expect("generation"));
    install_x11_scanout_surface(
        &mut state,
        x11_scanout_surface(
            354,
            width,
            height,
            SurfacePlacement::absolute_root_at(0, 0),
            DrmFormat::Xrgb8888,
        ),
        x11_output_snapshot(generation, 354, 354),
    );
    let window_id = state.window_id_for_surface(354).expect("candidate window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("candidate WindowGroup scene node");
    state.presentation_animator.set_enabled(true);
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                scene_node_id,
                PresentationRect::new(0.0, 0.0, 100.0, 100.0).expect("geometry start"),
                PresentationRect::new(1.0, 0.0, 100.0, 100.0).expect("geometry target"),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("candidate geometry transaction");

    assert!(
        state
            .direct_scanout_scene_blockers()
            .reasons()
            .contains(&DirectScanoutSceneRejection::AnimationTransform)
    );
}

#[test]
fn unrelated_off_output_geometry_track_does_not_block_direct_scanout() {
    let mut state = CompositorState::new(None);
    let (width, height) = (state.output_size.width, state.output_size.height);
    let generation = XwaylandGeneration::new(NonZeroU64::new(355).expect("generation"));
    install_x11_scanout_surface(
        &mut state,
        x11_scanout_surface(
            355,
            width,
            height,
            SurfacePlacement::absolute_root_at(0, 0),
            DrmFormat::Xrgb8888,
        ),
        x11_output_snapshot(generation, 355, 355),
    );
    let unrelated_node = install_off_output_xdg_window(&mut state, 356);
    state.presentation_animator.set_enabled(true);
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                unrelated_node,
                PresentationRect::new(0.0, 0.0, 16.0, 16.0).expect("geometry start"),
                PresentationRect::new(1.0, 0.0, 16.0, 16.0).expect("geometry target"),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("off-output geometry transaction");

    let blockers = state.direct_scanout_scene_blockers();
    assert!(
        !blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::AnimationTransform)
    );
    assert!(state.direct_scanout_scene_candidate().is_ok());
}

#[test]
fn unrelated_track_without_output_candidate_does_not_report_animation_blockers() {
    let mut state = CompositorState::new(None);
    let unrelated_node = install_off_output_xdg_window(&mut state, 357);
    state.presentation_animator.set_enabled(true);
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::mixed(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                unrelated_node,
                PresentationRect::new(0.0, 0.0, 16.0, 16.0).expect("geometry start"),
                PresentationRect::new(1.0, 0.0, 16.0, 16.0).expect("geometry target"),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
            vec![PresentationOpacityMutation::new(
                unrelated_node,
                PresentationOpacity::OPAQUE,
                PresentationOpacity::new(0.5).expect("half opacity"),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("off-output mixed transaction");

    let blockers = state.direct_scanout_scene_blockers();
    assert!(
        blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::NoOutputCoveringApplication)
    );
    assert!(
        !blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::AnimationTransform)
    );
    assert!(
        !blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::PresentationOpacity)
    );
}

#[test]
fn xwayland_backing_replacement_preserves_candidate_presentation_track_blockers() {
    let mut state = CompositorState::new(None);
    let (width, height) = (state.output_size.width, state.output_size.height);
    let generation = XwaylandGeneration::new(NonZeroU64::new(358).expect("generation"));
    let root_a = 358;
    let root_b = 359;
    let snapshot = x11_output_snapshot(generation, 3_580, root_a);
    let handle = snapshot.handle;
    install_x11_scanout_surface(
        &mut state,
        x11_scanout_surface(
            root_a,
            width,
            height,
            SurfacePlacement::absolute_root_at(0, 0),
            DrmFormat::Xrgb8888,
        ),
        snapshot,
    );
    let window_id = state
        .window_id_for_surface(root_a)
        .expect("candidate window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("candidate WindowGroup scene node");
    state.presentation_animator.set_enabled(true);
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::mixed(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                scene_node_id,
                PresentationRect::new(0.0, 0.0, 100.0, 100.0).expect("geometry start"),
                PresentationRect::new(1.0, 0.0, 100.0, 100.0).expect("geometry target"),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
            vec![PresentationOpacityMutation::new(
                scene_node_id,
                PresentationOpacity::OPAQUE,
                PresentationOpacity::new(0.5).expect("half opacity"),
                AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
            )],
        ))
        .expect("candidate presentation transaction");

    state.retire_xwayland_attachment(root_a);
    assert_eq!(state.attach_x11_surface(handle, root_b), Ok(Some(root_a)));
    state.append_renderable_surface(x11_scanout_surface(
        root_b,
        width,
        height,
        SurfacePlacement::absolute_root_at(0, 0),
        DrmFormat::Xrgb8888,
    ));
    state.surface_presentation_generations.insert(root_b, 1);
    state.rebuild_active_scene_view();

    assert_eq!(
        state.presentation_scene_node_id_for_root(root_b),
        Some(scene_node_id)
    );
    let blockers = state.direct_scanout_scene_blockers();
    assert!(
        blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::AnimationTransform)
    );
    assert!(
        blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::PresentationOpacity)
    );
}
