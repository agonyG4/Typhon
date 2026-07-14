use super::output::test_renderable_surface;
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReactiveHandoffOperation {
    PageflipComplete,
    Render,
    AtomicSubmit,
}

#[test]
fn matching_pageflip_and_queued_reactive_work_render_and_submit_in_one_cycle() {
    let mut scheduler = NativeFrameScheduler::new(165, 0);
    scheduler.note_async_submission(41, 1).unwrap();
    scheduler.queue_visual_work();
    let mut operations = Vec::new();

    assert!(matches!(
        scheduler.note_page_flip_completion(41, 6_060_606),
        PageFlipCompletionResult::Completed { .. }
    ));
    operations.push(ReactiveHandoffOperation::PageflipComplete);
    let decision = scheduler.decision_with_context(SchedulerFrameContext {
        pacing_mode: NativeOutputPacingMode::ReactiveDouble,
        capabilities: SchedulerCapabilities::explicit_atomic(true, true),
        presentation_target: None,
        predicted_total_cost: Duration::from_millis(100),
        now: MonotonicTimestampNs::new(6_060_606),
        render_target_available: true,
        render_ahead_allowed: false,
        ready_frame_present: false,
        ready_target_current: true,
    });
    assert_eq!(decision, SchedulerDecision::Render);
    operations.push(ReactiveHandoffOperation::Render);
    scheduler.note_async_submission(42, 6_060_607).unwrap();
    operations.push(ReactiveHandoffOperation::AtomicSubmit);

    assert_eq!(
        operations,
        [
            ReactiveHandoffOperation::PageflipComplete,
            ReactiveHandoffOperation::Render,
            ReactiveHandoffOperation::AtomicSubmit,
        ]
    );
    assert!(scheduler.page_flip_pending());
}
use crate::native_output::runtime::{
    NativeRepaintDecision, NativeRepaintInputs, native_repaint_decision,
};
use oblivion_one::native::scheduler::SchedulerCapabilities;

#[test]
fn render_ahead_requires_atomic_in_fence_support() {
    let eligible = SchedulerCapabilities::explicit_atomic(true, true);
    let missing_in_fence = SchedulerCapabilities::explicit_atomic(false, true);
    let opaque_output = SchedulerCapabilities::explicit_atomic(true, false);
    let legacy = SchedulerCapabilities::legacy();

    assert!(eligible.render_ahead_allowed());
    assert!(!missing_in_fence.render_ahead_allowed());
    assert!(!opaque_output.render_ahead_allowed());
    assert!(!legacy.render_ahead_allowed());
}

#[test]
fn native_xrgb_copy_preserves_ignored_high_byte_for_fast_row_copy() {
    let frame = [0x7f11_2233];
    let mut bytes = [0; 4];

    copy_argb_frame_to_xrgb_mapping(&frame, 1, 1, 4, &mut bytes).unwrap();

    assert_eq!(bytes, 0x7f11_2233u32.to_ne_bytes());
}

#[test]
fn native_xrgb_copy_damage_updates_only_requested_rectangles() {
    let frame = [0xff00_0001, 0xff00_0002, 0xff00_0003, 0xff00_0004];
    let untouched = 0xa5;
    let mut bytes = [untouched; 16];

    let copied = copy_argb_frame_to_xrgb_mapping_damage(
        &frame,
        2,
        2,
        8,
        &mut bytes,
        NativeFrameCopyDamage::Rects(&[NativeDamageRect {
            x: 1,
            y: 0,
            width: 1,
            height: 2,
        }]),
    )
    .unwrap();

    assert_eq!(copied, 8);
    assert_eq!(&bytes[0..4], &[untouched; 4]);
    assert_eq!(&bytes[4..8], &0xff00_0002u32.to_ne_bytes());
    assert_eq!(&bytes[8..12], &[untouched; 4]);
    assert_eq!(&bytes[12..16], &0xff00_0004u32.to_ne_bytes());
}

#[test]
fn native_xrgb_copy_damage_caps_overlapping_rects_at_full_frame_copy() {
    let frame = [0xff00_0001, 0xff00_0002, 0xff00_0003, 0xff00_0004];
    let mut bytes = [0; 16];

    let copied = copy_argb_frame_to_xrgb_mapping_damage(
        &frame,
        2,
        2,
        8,
        &mut bytes,
        NativeFrameCopyDamage::Rects(&[
            NativeDamageRect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
            NativeDamageRect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
        ]),
    )
    .unwrap();

    assert_eq!(copied, 16);
    assert_eq!(&bytes[0..4], &0xff00_0001u32.to_ne_bytes());
    assert_eq!(&bytes[4..8], &0xff00_0002u32.to_ne_bytes());
    assert_eq!(&bytes[8..12], &0xff00_0003u32.to_ne_bytes());
    assert_eq!(&bytes[12..16], &0xff00_0004u32.to_ne_bytes());
}

#[test]
fn native_frame_renderer_repairs_surface_bounds_change_with_partial_scene_rebuild() {
    let mut renderer = NativeFrameRenderer::default();
    let initial_surface = test_renderable_surface(7, 0, 0, 4, 4, RenderableSurfaceDamage::Full);

    let initial = renderer.render_frame(NativeFrameRequest {
        width: 96,
        height: 96,
        surfaces: &[initial_surface],
        external_overlay_surface_ids: Vec::new(),
        visual_state: DesktopVisualState::wallpaper_only(),
        render_generation: 1,
        client_cursor: None,
    });
    assert_eq!(initial.scene_rebuild_kind, DesktopSceneRebuildKind::Full);

    let moved_surface = test_renderable_surface(
        7,
        2,
        0,
        4,
        4,
        RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }]),
    );

    let moved = renderer.render_frame(NativeFrameRequest {
        width: 96,
        height: 96,
        surfaces: &[moved_surface],
        external_overlay_surface_ids: Vec::new(),
        visual_state: DesktopVisualState::wallpaper_only(),
        render_generation: 2,
        client_cursor: None,
    });

    assert_eq!(moved.scene_rebuild_kind, DesktopSceneRebuildKind::Partial);
    assert_eq!(moved.frame_copy_kind, DesktopFrameCopyKind::Partial);
}

#[test]
fn native_frame_renderer_reports_full_scene_rebuild_when_surface_identity_changes() {
    let mut renderer = NativeFrameRenderer::default();
    let initial_surface = test_renderable_surface(7, 0, 0, 4, 4, RenderableSurfaceDamage::Full);

    let initial = renderer.render_frame(NativeFrameRequest {
        width: 96,
        height: 96,
        surfaces: &[initial_surface],
        external_overlay_surface_ids: Vec::new(),
        visual_state: DesktopVisualState::wallpaper_only(),
        render_generation: 1,
        client_cursor: None,
    });
    assert_eq!(initial.scene_rebuild_kind, DesktopSceneRebuildKind::Full);

    let replacement_surface = test_renderable_surface(
        8,
        0,
        0,
        4,
        4,
        RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }]),
    );

    let replacement = renderer.render_frame(NativeFrameRequest {
        width: 96,
        height: 96,
        surfaces: &[replacement_surface],
        external_overlay_surface_ids: Vec::new(),
        visual_state: DesktopVisualState::wallpaper_only(),
        render_generation: 2,
        client_cursor: None,
    });

    assert_eq!(
        replacement.scene_rebuild_kind,
        DesktopSceneRebuildKind::Full
    );
}

#[test]
fn native_cursor_argb_bytes_places_texture_pixels_in_pitched_buffer() {
    let pixels = [0xff11_2233, 0x8044_5566, 0xff77_8899, 0];

    let bytes = native_cursor_argb_bytes(&pixels, 2, 2, 4, 4, 16).unwrap();

    assert_eq!(&bytes[0..4], &0xff11_2233u32.to_ne_bytes());
    assert_eq!(&bytes[4..8], &0x8044_5566u32.to_ne_bytes());
    assert_eq!(&bytes[16..20], &0xff77_8899u32.to_ne_bytes());
    assert_eq!(&bytes[20..24], &0u32.to_ne_bytes());
    assert!(bytes[24..].iter().all(|byte| *byte == 0));
}

#[test]
fn native_input_coalesces_consecutive_relative_motion_events() {
    let events = coalesce_pointer_motion_events(vec![
        NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
            10,
            RelativeMotion {
                dx: 1.0,
                dy: 0.0,
                dx_unaccelerated: 2.0,
                dy_unaccelerated: 0.0,
            },
        )),
        NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
            20,
            RelativeMotion {
                dx: 0.0,
                dy: 2.0,
                dx_unaccelerated: 0.0,
                dy_unaccelerated: 3.0,
            },
        )),
        NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
            30,
            RelativeMotion {
                dx: 3.0,
                dy: 4.0,
                dx_unaccelerated: 5.0,
                dy_unaccelerated: 6.0,
            },
        )),
    ]);

    assert_eq!(
        events,
        vec![NativeHardwareInputEvent::PointerMotion(
            PointerMotionSample::relative(
                30,
                RelativeMotion {
                    dx: 4.0,
                    dy: 6.0,
                    dx_unaccelerated: 7.0,
                    dy_unaccelerated: 9.0,
                },
            )
        )]
    );
}

#[test]
fn native_pointer_motion_sample_keeps_relative_delta_when_cursor_clamps_at_edge() {
    let mut input = NativeInputState::new(320, 200);
    let sample = PointerMotionSample::relative(
        42,
        RelativeMotion {
            dx: 1_000.0,
            dy: -1_000.0,
            dx_unaccelerated: 1_200.0,
            dy_unaccelerated: -1_200.0,
        },
    );

    let effect = input.handle_hardware_input_event(NativeHardwareInputEvent::PointerMotion(sample));

    assert_eq!(effect.pointer_motion, Some((319.0, 0.0)));
    assert_eq!(effect.relative_motion, Some(sample.relative.unwrap()));
}

#[test]
fn native_input_coalescing_preserves_button_boundaries() {
    let events = coalesce_pointer_motion_events(vec![
        NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
            10,
            RelativeMotion::accelerated_only(1.0, 0.0),
        )),
        NativeHardwareInputEvent::PointerButton {
            button: u32::from(BTN_LEFT),
            pressed: true,
        },
        NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
            20,
            RelativeMotion::accelerated_only(0.0, 2.0),
        )),
    ]);

    assert_eq!(
        events,
        vec![
            NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
                10,
                RelativeMotion::accelerated_only(1.0, 0.0),
            )),
            NativeHardwareInputEvent::PointerButton {
                button: u32::from(BTN_LEFT),
                pressed: true,
            },
            NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
                20,
                RelativeMotion::accelerated_only(0.0, 2.0),
            )),
        ]
    );
}

#[test]
fn native_input_coalesces_consecutive_absolute_motion_to_latest_position() {
    let events = coalesce_pointer_motion_events(vec![
        NativeHardwareInputEvent::PointerMotion(PointerMotionSample::absolute(10, 12.0, 30.0)),
        NativeHardwareInputEvent::PointerMotion(PointerMotionSample::absolute(20, 18.0, 35.0)),
    ]);

    assert_eq!(
        events,
        vec![NativeHardwareInputEvent::PointerMotion(
            PointerMotionSample::absolute(20, 18.0, 35.0)
        )]
    );
}

#[test]
fn input_event_paths_select_only_real_keyboard_and_mouse_devices() {
    let root = std::env::current_dir()
        .unwrap()
        .join("target")
        .join("native-input-tests")
        .join(std::process::id().to_string());
    let dev_root = root.join("dev-input");
    let udev_root = root.join("udev-data");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&dev_root).unwrap();
    fs::create_dir_all(&udev_root).unwrap();
    fs::write(dev_root.join("event3"), "").unwrap();
    fs::write(dev_root.join("event4"), "").unwrap();
    fs::write(dev_root.join("event12"), "").unwrap();
    fs::write(udev_root.join("c13:67"), "E:ID_INPUT_MOUSE=1\n").unwrap();
    fs::write(udev_root.join("c13:68"), "E:ID_INPUT_KEYBOARD=1\n").unwrap();
    fs::write(udev_root.join("c13:76"), "E:ID_INPUT=1\n").unwrap();

    let paths = input_event_paths_with_udev(&dev_root, &udev_root);
    let names = paths
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&root);

    assert_eq!(names, ["event3", "event4"]);
}

#[test]
fn native_repaint_decision_skips_visible_frame_callback_without_damage() {
    assert_eq!(
        native_repaint_decision(NativeRepaintInputs {
            accepted_clients: false,
            render_generation_changed: false,
            pending_frame_work: true,
            only_pending_surface_frame_callbacks: true,
            redraw_requested: false,
            page_flip_pending: false,
        }),
        NativeRepaintDecision {
            repaint: false,
            protocol_only_present: true,
        }
    );
}

#[test]
fn native_repaint_decision_paints_non_callback_pending_frame_work() {
    assert_eq!(
        native_repaint_decision(NativeRepaintInputs {
            accepted_clients: false,
            render_generation_changed: false,
            pending_frame_work: true,
            only_pending_surface_frame_callbacks: false,
            redraw_requested: false,
            page_flip_pending: false,
        }),
        NativeRepaintDecision {
            repaint: true,
            protocol_only_present: false,
        }
    );
}

#[test]
fn native_repaint_decision_paints_visual_changes_even_with_frame_callback() {
    assert_eq!(
        native_repaint_decision(NativeRepaintInputs {
            accepted_clients: false,
            render_generation_changed: true,
            pending_frame_work: true,
            only_pending_surface_frame_callbacks: true,
            redraw_requested: false,
            page_flip_pending: false,
        }),
        NativeRepaintDecision {
            repaint: true,
            protocol_only_present: false,
        }
    );
}

#[test]
fn native_repaint_decision_waits_for_pending_pageflip_before_repaint() {
    assert_eq!(
        native_repaint_decision(NativeRepaintInputs {
            accepted_clients: false,
            render_generation_changed: true,
            pending_frame_work: true,
            only_pending_surface_frame_callbacks: false,
            redraw_requested: true,
            page_flip_pending: true,
        }),
        NativeRepaintDecision {
            repaint: false,
            protocol_only_present: false,
        }
    );
}
