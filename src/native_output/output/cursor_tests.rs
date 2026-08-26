use super::*;
use crate::native_output::OutputTransactionId;
use crate::native_output::runtime::{
    NativeCursorOutputArbitration, NativeCursorRenderMode, observe_atomic_cursor_output_liveness,
    update_cursor_output_arbitration,
};
use oblivion_one::native::kms::{
    AtomicPlaneProperties, DrmFormatModifierPair, PlanePropertyId, PropertyId,
};
use oblivion_one::native::scheduler::NativeFrameScheduler;

fn property(id: u32) -> PlanePropertyId {
    PlanePropertyId(PropertyId::new(id).expect("test property id is nonzero"))
}

fn test_cursor() -> NativeAtomicCursor {
    let state = AtomicCursorVisualState::hidden(64, 64);
    let mapping = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            1,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    assert_ne!(mapping, libc::MAP_FAILED);
    NativeAtomicCursor {
        image: Arc::new(CompositorCursorImage::builtin_fallback()),
        theme_image: Arc::new(CompositorCursorImage::builtin_fallback()),
        source_key: NativeCursorSourceKey::Theme,
        desired: state.clone(),
        submitted: state.clone(),
        current: state,
        resources: AtomicCursorResources {
            current: Some(AtomicCursorBuffer {
                fd: -1,
                handle: 1,
                framebuffer: FramebufferId::new(91).unwrap(),
                width: 64,
                height: 64,
                pitch: 256,
                size: 1,
                mapping,
                drm_cleanup_armed: false,
                image_owner: None,
                lease: Arc::new(()),
            }),
            retired: Vec::new(),
            theme_cache: None,
            client_cache: None,
        },
        plane: AtomicCursorPlaneProperties {
            plane_id: 4,
            crtc_id: 2,
            fb_id: 1,
            crtc_x: 2,
            crtc_y: 3,
            crtc_w: 4,
            crtc_h: 5,
            src_x: 6,
            src_y: 7,
            src_w: 8,
            src_h: 9,
            in_formats: None,
            rotation: None,
            property_ids: AtomicPlaneProperties {
                fb_id: property(10),
                crtc_id: property(11),
                src_x: property(12),
                src_y: property(13),
                src_w: property(14),
                src_h: property(15),
                crtc_x: property(16),
                crtc_y: property(17),
                crtc_w: property(18),
                crtc_h: property(19),
                plane_type: property(20),
                in_fence_fd: None,
                in_formats: None,
                in_formats_async: None,
                damage_clips: None,
                rotation: None,
                alpha: None,
                pixel_blend_mode: None,
                color_encoding: None,
                color_range: None,
            },
            format_modifier: DrmFormatModifierPair {
                fourcc: DRM_FORMAT_ARGB8888,
                modifier: 0,
            },
            alpha_maximum: None,
            pixel_blend_mode_premultiplied: None,
        },
        generation: 1,
        output_transform: 0,
        output_scale_milli: 1_000,
        desired_epoch: INITIAL_CURSOR_EPOCH,
        submitted_epoch: INITIAL_CURSOR_EPOCH,
        revisions: CursorRevisionTracker::new(),
        hardware_path_active: false,
        dirty: AtomicCursorDirty::default(),
        counters: AtomicCursorCounters::default(),
        plane_lifecycle: CursorPlaneLifecycle::new(1),
        capability_cache: Default::default(),
        scheduled_test_policy: KmsCursorTestPolicy::Required,
        crtc_id: 2,
        mode_width: 1920,
        mode_height: 1080,
        client_image_failure: None,
        pending_token: None,
        pending_is_primary: false,
        worker_queued: None,
        suspended_desired: None,
        drm_cleanup_armed: false,
    }
}

pub(super) fn test_cursor_for_worker() -> NativeAtomicCursor {
    test_cursor()
}

#[test]
fn queueing_cursor_job_does_not_advance_last_submitted_epoch() {
    let mut cursor = test_cursor();
    cursor.set_hardware_path_active(true);
    let epoch = cursor.desired_epoch();
    let transaction_id = OutputTransactionId::new(
        std::num::NonZeroU64::new(77).expect("test transaction ID is nonzero"),
    );
    let token = PageFlipToken::new(77).unwrap();

    cursor
        .queue_worker_submission(transaction_id, token, epoch, cursor.desired().clone())
        .unwrap();

    assert_eq!(cursor.submitted_epoch(), INITIAL_CURSOR_EPOCH);
    assert_eq!(cursor.worker_queued_epoch(), Some(epoch));
}

#[test]
fn worker_queue_rejects_a_stale_cursor_epoch() {
    let mut cursor = test_cursor();
    let stale_epoch = cursor.desired_epoch();
    cursor.set_position(100, 200);
    let transaction_id = OutputTransactionId::new(
        std::num::NonZeroU64::new(76).expect("test transaction ID is nonzero"),
    );
    let token = PageFlipToken::new(76).unwrap();

    assert!(
        cursor
            .queue_worker_submission(transaction_id, token, stale_epoch, cursor.desired().clone(),)
            .is_err()
    );
    assert!(cursor.worker_queued_epoch().is_none());
}

#[test]
fn owned_cursor_snapshot_queues_after_desired_motion_advances() {
    let mut cursor = test_cursor();
    cursor.set_hardware_path_active(true);
    let snapshot_epoch = cursor.desired_epoch();
    let snapshot_revision = cursor.desired_revision();
    let snapshot_state = cursor.desired().clone();
    cursor.set_position(100, 200);
    let transaction_id = OutputTransactionId::new(
        std::num::NonZeroU64::new(75).expect("test transaction ID is nonzero"),
    );
    let token = PageFlipToken::new(75).unwrap();

    cursor
        .queue_owned_worker_submission(
            transaction_id,
            token,
            snapshot_epoch,
            snapshot_revision,
            snapshot_state,
        )
        .expect("an immutable owned snapshot remains queueable after newer desired motion");

    assert_eq!(cursor.worker_queued_epoch(), Some(snapshot_epoch));
}

#[test]
fn cursor_output_liveness_uses_kms_visible_state() {
    let mut cursor = test_cursor();

    assert!(!cursor.needs_output_liveness());

    cursor.set_position(100, 200);
    assert!(!cursor.needs_output_liveness());

    cursor.current = cursor.desired.clone();
    cursor.set_visible(true);
    assert!(cursor.needs_output_liveness());

    cursor.current = cursor.desired.clone();
    cursor.set_position(120, 220);
    assert!(cursor.needs_output_liveness());
}

#[test]
fn cursor_output_liveness_uses_worker_owned_state() {
    let mut cursor = test_cursor();
    cursor.current.visible = true;
    cursor.desired.visible = true;
    cursor.set_position(100, 200);
    let queued_epoch = cursor.desired_epoch();
    let queued_state = cursor.desired().clone();
    let transaction_id = OutputTransactionId::new(
        std::num::NonZeroU64::new(81).expect("test transaction ID is nonzero"),
    );
    let token = PageFlipToken::new(81).unwrap();

    cursor
        .queue_worker_submission(transaction_id, token, queued_epoch, queued_state)
        .unwrap();
    cursor.set_position(0, 0);

    assert!(cursor.needs_output_liveness());
}

#[test]
fn cursor_output_liveness_survives_a_newer_state_during_inflight_submission() {
    let mut cursor = test_cursor();
    cursor.current.visible = true;
    cursor.desired.visible = true;
    cursor.set_position(100, 200);
    let submitted_epoch = cursor.desired_epoch();
    let submitted_state = cursor.desired().clone();
    let submitted_revision = cursor.desired_revision();
    let token = PageFlipToken::new(82).unwrap();

    cursor.begin_submission_at_revision_with_capability_key(
        token,
        submitted_state,
        submitted_epoch,
        submitted_revision,
        None,
    );
    cursor.set_position(0, 0);

    assert!(cursor.needs_output_liveness());
}

#[test]
fn input_boundary_observer_arms_one_scheduler_derived_cursor_window() {
    let mut cursor = test_cursor();
    cursor.set_visible(true);
    cursor.current = cursor.desired.clone();
    cursor.set_position(100, 200);
    let scheduler = NativeFrameScheduler::new(165, 0);
    let mut arbitration = NativeCursorOutputArbitration::default();
    let scene_redraw_requested = false;

    assert!(observe_atomic_cursor_output_liveness(
        Some(&cursor),
        &mut arbitration,
        &scheduler,
        1_000,
        NativeCursorRenderMode::Hardware,
        true,
    ));
    assert!(arbitration.pending());
    assert_eq!(
        arbitration.deadline_ns(),
        Some(scheduler.next_refresh_deadline_ns(1_000))
    );
    assert!(!scene_redraw_requested);

    let first_deadline = arbitration.deadline_ns();
    cursor.set_position(120, 220);
    assert!(observe_atomic_cursor_output_liveness(
        Some(&cursor),
        &mut arbitration,
        &scheduler,
        1_001,
        NativeCursorRenderMode::Hardware,
        true,
    ));
    assert_eq!(arbitration.deadline_ns(), first_deadline);
    assert_eq!(arbitration.response_windows_opened(), 1);
    assert_eq!(arbitration.changes_coalesced(), 1);
}

#[test]
fn input_boundary_observer_cancels_stale_atomic_debt_before_submission() {
    let mut cursor = test_cursor();
    cursor.set_visible(true);
    cursor.current = cursor.desired.clone();
    cursor.set_position(100, 200);
    let scheduler = NativeFrameScheduler::new(165, 0);
    let mut arbitration = NativeCursorOutputArbitration::default();

    assert!(observe_atomic_cursor_output_liveness(
        Some(&cursor),
        &mut arbitration,
        &scheduler,
        1_000,
        NativeCursorRenderMode::Hardware,
        true,
    ));
    cursor.set_position(0, 0);

    assert!(!observe_atomic_cursor_output_liveness(
        Some(&cursor),
        &mut arbitration,
        &scheduler,
        1_001,
        NativeCursorRenderMode::Hardware,
        true,
    ));
    assert!(!arbitration.pending());
}

#[test]
fn software_cursor_only_arms_atomic_liveness_to_clear_a_visible_plane() {
    let mut cursor = test_cursor();
    cursor.current.visible = true;
    cursor.desired.visible = true;
    let scheduler = NativeFrameScheduler::new(165, 0);
    let mut arbitration = NativeCursorOutputArbitration::default();

    assert!(observe_atomic_cursor_output_liveness(
        Some(&cursor),
        &mut arbitration,
        &scheduler,
        1_000,
        NativeCursorRenderMode::Software,
        true,
    ));
    assert!(arbitration.pending());

    arbitration.clear_pending();
    cursor.current.visible = false;
    assert!(!observe_atomic_cursor_output_liveness(
        Some(&cursor),
        &mut arbitration,
        &scheduler,
        1_001,
        NativeCursorRenderMode::Software,
        true,
    ));
    assert!(!arbitration.pending());
}

#[test]
fn input_boundary_liveness_rearms_after_an_older_inflight_cursor_completes() {
    let mut cursor = test_cursor();
    cursor.current.visible = true;
    cursor.desired.visible = true;
    cursor.set_position(100, 200);
    let submitted_epoch = cursor.desired_epoch();
    let submitted_state = cursor.desired().clone();
    let submitted_revision = cursor.desired_revision();
    let token = PageFlipToken::new(83).unwrap();

    cursor.begin_submission_at_revision_with_capability_key(
        token,
        submitted_state,
        submitted_epoch,
        submitted_revision,
        None,
    );
    cursor.set_position(0, 0);
    let newer_epoch = cursor.desired_epoch();
    let scheduler = NativeFrameScheduler::new(165, 0);
    let mut arbitration = NativeCursorOutputArbitration::default();

    assert!(observe_atomic_cursor_output_liveness(
        Some(&cursor),
        &mut arbitration,
        &scheduler,
        1_000,
        NativeCursorRenderMode::Hardware,
        true,
    ));
    assert_eq!(arbitration.desired_epoch(), newer_epoch);

    cursor
        .complete_submission(token, cursor.generation)
        .expect("the older cursor submission completes");
    arbitration.consume_submitted_epoch(
        submitted_epoch,
        2_000,
        scheduler.next_refresh_deadline_ns(2_000),
    );

    assert!(arbitration.pending());
    assert_eq!(arbitration.desired_epoch(), newer_epoch);
    assert!(arbitration.deadline_ns().is_some());
}

#[test]
fn composed_cursor_fallback_counter_is_separate_from_general_fallbacks() {
    let mut cursor = test_cursor();

    cursor.note_composed_software_fallback();

    assert_eq!(cursor.counters.software_fallbacks, 0);
    assert_eq!(cursor.counters.composed_cursor_fallbacks, 1);
}

#[test]
fn queued_cursor_assignment_pins_the_exact_framebuffer_resource() {
    let cursor = test_cursor();
    let state = AtomicCursorVisualState {
        framebuffer_id: Some(91),
        visible: true,
        ..AtomicCursorVisualState::hidden(64, 64)
    };

    let pin = cursor.pin_framebuffer_for(&state).unwrap();
    assert_eq!(pin.framebuffer_id().get(), 91);
    assert!(pin.is_job_owned());
}

#[test]
fn pinned_cursor_resource_survives_retirement_until_job_release() {
    let mut cursor = test_cursor();
    let state = AtomicCursorVisualState {
        framebuffer_id: Some(91),
        visible: true,
        ..AtomicCursorVisualState::hidden(64, 64)
    };
    let pin = cursor.pin_framebuffer_for(&state).unwrap();
    let old = cursor.resources.current.take().unwrap();
    cursor.resources.retired.push(old);
    cursor.resources.retire_safe(&[]);
    assert_eq!(cursor.resources.retired.len(), 1);
    drop(pin);
    cursor.resources.retire_safe(&[]);
    assert!(cursor.resources.retired.is_empty());
}

#[test]
fn worker_cursor_success_advances_exact_epoch_once() {
    let mut cursor = test_cursor();
    cursor.set_hardware_path_active(true);
    let queued_epoch = cursor.desired_epoch();
    let transaction_id = OutputTransactionId::new(
        std::num::NonZeroU64::new(78).expect("test transaction ID is nonzero"),
    );
    let token = PageFlipToken::new(78).unwrap();
    cursor
        .queue_worker_submission(
            transaction_id,
            token,
            queued_epoch,
            cursor.desired().clone(),
        )
        .unwrap();
    cursor.set_position(100, 200);
    let newer_epoch = cursor.desired_epoch();

    let queued = cursor
        .take_worker_submission(transaction_id, token, queued_epoch)
        .unwrap();
    cursor.begin_submission_at_revision_with_capability_key(
        token,
        queued.visual_state,
        queued.cursor_epoch,
        queued.revision,
        queued.capability_key,
    );

    assert_eq!(cursor.submitted_epoch(), queued_epoch);
    assert_ne!(cursor.submitted_epoch(), newer_epoch);
    assert!(cursor.worker_queued_epoch().is_none());
}

#[test]
fn desired_queued_submitted_and_current_cursor_states_advance_at_exact_boundaries() {
    let mut cursor = test_cursor();
    cursor.set_position(100, 200);
    cursor.set_visible(true);
    cursor.set_hardware_path_active(true);
    let desired = cursor.desired().clone();
    let desired_epoch = cursor.desired_epoch();
    let transaction_id = OutputTransactionId::new(
        std::num::NonZeroU64::new(79).expect("test transaction ID is nonzero"),
    );
    let token = PageFlipToken::new(79).unwrap();

    cursor
        .queue_worker_submission(transaction_id, token, desired_epoch, desired.clone())
        .unwrap();
    assert!(!cursor.current().visible);
    assert_eq!(cursor.submitted_epoch(), INITIAL_CURSOR_EPOCH);

    let queued = cursor
        .take_worker_submission(transaction_id, token, desired_epoch)
        .unwrap();
    let submitted_revision = queued.revision;
    cursor.begin_submission_at_revision_with_capability_key(
        token,
        queued.visual_state,
        queued.cursor_epoch,
        queued.revision,
        queued.capability_key,
    );
    assert!(!cursor.current().visible);
    assert_eq!(cursor.submitted_epoch(), desired_epoch);

    let stale_token = PageFlipToken::new(80).unwrap();
    assert!(
        cursor
            .complete_submission(stale_token, cursor.generation)
            .is_err()
    );
    assert!(!cursor.current().visible);
    assert!(
        cursor
            .complete_submission(token, cursor.generation + 1)
            .is_err()
    );
    assert!(!cursor.current().visible);

    cursor
        .complete_submission(token, cursor.generation)
        .expect("exact pageflip promotes submitted cursor state");
    assert_eq!(cursor.current(), &desired);
    assert_eq!(
        cursor.presented_plane_state().revision,
        submitted_revision,
        "the pageflip must promote the exact queued typed revision"
    );
    assert!(
        cursor
            .complete_submission(token, cursor.generation)
            .is_err()
    );
}

#[test]
fn hidden_cursor_position_changes_do_not_need_submission() {
    let mut cursor = test_cursor();
    cursor.set_position(100, 200);

    assert!(!cursor.needs_submission());
}

#[test]
fn redundant_position_does_not_advance_cursor_epoch() {
    let mut cursor = test_cursor();
    let initial_epoch = cursor.desired_epoch();

    cursor.set_position(0, 0);

    assert_eq!(cursor.desired_epoch(), initial_epoch);
}

#[test]
fn new_position_advances_cursor_epoch_once() {
    let mut cursor = test_cursor();
    let initial_epoch = cursor.desired_epoch();

    cursor.set_position(100, 200);
    assert_ne!(cursor.desired_epoch(), initial_epoch);
    let moved_epoch = cursor.desired_epoch();

    cursor.set_position(100, 200);

    assert_eq!(cursor.desired_epoch(), moved_epoch);
}

#[test]
fn cursor_revision_advances_only_the_changed_field() {
    let mut cursor = test_cursor();
    let initial = cursor.desired_revision();

    cursor.set_position(100, 200);
    let moved = cursor.desired_revision();
    assert_eq!(moved.image, initial.image);
    assert_ne!(moved.motion, initial.motion);
    assert_eq!(moved.visibility, initial.visibility);

    cursor.set_visible(true);
    let visible = cursor.desired_revision();
    assert_eq!(visible.image, moved.image);
    assert_eq!(visible.motion, moved.motion);
    assert_ne!(visible.visibility, moved.visibility);
}

#[test]
fn visibility_change_advances_cursor_epoch_once() {
    let mut cursor = test_cursor();
    let initial_epoch = cursor.desired_epoch();

    cursor.set_visible(true);
    assert_ne!(cursor.desired_epoch(), initial_epoch);
    let visible_epoch = cursor.desired_epoch();

    cursor.set_visible(true);

    assert_eq!(cursor.desired_epoch(), visible_epoch);
}

#[test]
fn hardware_path_transition_advances_cursor_epoch_once() {
    let mut cursor = test_cursor();
    let initial_epoch = cursor.desired_epoch();

    cursor.set_hardware_path_active(true);
    assert_ne!(cursor.desired_epoch(), initial_epoch);
    let active_epoch = cursor.desired_epoch();

    cursor.set_hardware_path_active(true);
    assert_eq!(cursor.desired_epoch(), active_epoch);

    cursor.set_hardware_path_active(false);
    assert_ne!(cursor.desired_epoch(), active_epoch);
}

#[test]
fn cursor_epoch_wrap_skips_zero_and_submitted_epoch() {
    let mut cursor = test_cursor();
    cursor.desired_epoch = u64::MAX;
    cursor.submitted_epoch = 1;

    cursor.set_position(100, 200);

    assert_eq!(cursor.desired_epoch(), 2);
}

#[test]
fn idle_theme_cursor_motion_opens_plane_delta_deadline_without_scene_damage() {
    let mut cursor = test_cursor();
    cursor.desired.visible = true;
    cursor.current.visible = true;
    cursor.set_position(100, 200);
    assert!(cursor.needs_submission());

    let mut arbitration = NativeCursorOutputArbitration::default();
    let scheduler = NativeFrameScheduler::new(165, 0);
    let (cursor_changed, deadline_due, cursor_work_pending) = update_cursor_output_arbitration(
        &mut arbitration,
        cursor.desired_epoch(),
        INITIAL_CURSOR_EPOCH,
        1_000,
        &scheduler,
        false,
        true,
    );

    assert!(cursor_changed);
    assert!(!deadline_due);
    assert!(!cursor_work_pending);
    let deadline = arbitration.deadline_ns().expect("cursor deadline is armed");
    let (_, deadline_due, cursor_work_pending) = update_cursor_output_arbitration(
        &mut arbitration,
        cursor.desired_epoch(),
        INITIAL_CURSOR_EPOCH,
        deadline,
        &scheduler,
        false,
        true,
    );
    assert!(deadline_due);
    assert!(cursor_work_pending);
}

#[test]
fn atomic_cursor_state_uses_theme_hotspot() {
    let image = CompositorCursorImage::from_argb8888(vec![0xffff_ffff; 2 * 3], 2, 3, 1, 2).unwrap();
    let state = atomic_cursor_state_for_image(&image, Some(7));

    assert_eq!(state.hotspot_x, 1);
    assert_eq!(state.hotspot_y, 2);
    assert_eq!((state.width, state.height), (2, 3));
    assert_eq!(state.framebuffer_id, Some(7));
}

#[test]
fn oversized_theme_cursor_falls_back_to_software_in_auto() {
    let image =
        CompositorCursorImage::from_argb8888(vec![0xffff_ffff; 65 * 64], 65, 64, 0, 0).unwrap();
    assert!(!cursor_image_fits_buffer(
        &image,
        NATIVE_HARDWARE_CURSOR_SIZE,
        64
    ));
}

#[test]
fn one_hundred_oversized_hardware_replacements_remain_software_eligible() {
    let image =
        CompositorCursorImage::from_argb8888(vec![0xffff_ffff; 65 * 64], 65, 64, 0, 0).unwrap();
    for _ in 0..100 {
        assert!(!cursor_image_fits_buffer(
            &image,
            NATIVE_HARDWARE_CURSOR_SIZE,
            64
        ));
        assert!(validate_atomic_cursor_image(&image, NATIVE_HARDWARE_CURSOR_SIZE, 64).is_err());
    }
}

#[test]
fn oversized_theme_cursor_fails_in_hardware_mode() {
    let image =
        CompositorCursorImage::from_argb8888(vec![0xffff_ffff; 65 * 64], 65, 64, 0, 0).unwrap();
    assert!(validate_atomic_cursor_image(&image, NATIVE_HARDWARE_CURSOR_SIZE, 64).is_err());
}

#[test]
fn client_cursor_transform_preserves_pixel_orientation_and_hotspot() {
    use wayland_server::protocol::wl_output::Transform;

    let (pixels, (width, height)) =
        transform_cursor_pixels(&[0, 1, 2, 3, 4, 5], 2, 3, Transform::_90).unwrap();
    assert_eq!((width, height), (3, 2));
    assert_eq!(pixels, vec![4, 2, 0, 5, 3, 1]);
    assert_eq!(
        normalize_cursor_hotspot(1, 2, 2, 3, 3, 2, 3, 2, Transform::_90),
        Some((0, 1))
    );
}

#[test]
fn client_cursor_hotspot_outside_source_is_rejected() {
    use wayland_server::protocol::wl_output::Transform;

    assert_eq!(
        normalize_cursor_hotspot(2, 0, 2, 2, 2, 2, 2, 2, Transform::Normal),
        None
    );
}

#[test]
fn client_cursor_image_key_changes_for_commit_and_hotspot() {
    let first = NativeCursorImageKey {
        surface_id: 7,
        buffer_id: 11,
        commit_sequence: 3,
        hotspot_x: 1,
        hotspot_y: 2,
        width: 32,
        height: 32,
        buffer_scale: 1,
        buffer_transform: 0,
        output_scale_milli: 1_000,
    };
    let mut next = first;
    next.commit_sequence += 1;
    assert_ne!(first, next);
    next = first;
    next.hotspot_x += 1;
    assert_ne!(first, next);
}

#[test]
fn client_cursor_image_key_changes_for_output_scale() {
    let first = NativeCursorImageKey {
        surface_id: 7,
        buffer_id: 11,
        commit_sequence: 3,
        hotspot_x: 1,
        hotspot_y: 2,
        width: 32,
        height: 32,
        buffer_scale: 1,
        buffer_transform: 0,
        output_scale_milli: 1_000,
    };
    let mut next = first;
    next.output_scale_milli = 1_250;

    assert_ne!(first, next);
}

#[test]
fn hidden_cursor_image_changes_do_not_need_submission() {
    let mut cursor = test_cursor();
    cursor.desired.framebuffer_id = Some(99);
    cursor.desired.image_generation = 2;
    cursor.dirty.image = true;

    assert!(!cursor.needs_submission());
}

#[test]
fn hidden_to_visible_submits_latest_position() {
    let mut cursor = test_cursor();
    cursor.set_position(100, 200);
    cursor.set_visible(true);

    assert!(cursor.needs_submission());
    assert_eq!(cursor.desired().x, 100);
    assert_eq!(cursor.desired().y, 200);
}

#[test]
fn visible_to_hidden_submits_plane_disable() {
    let mut cursor = test_cursor();
    cursor.desired.visible = true;
    cursor.current.visible = true;
    cursor.set_visible(false);

    assert!(cursor.needs_submission());
}

#[test]
fn visible_cursor_position_change_needs_submission() {
    let mut cursor = test_cursor();
    cursor.desired.visible = true;
    cursor.current.visible = true;
    cursor.set_position(100, 200);

    assert!(cursor.needs_submission());
}

#[test]
fn capability_quarantine_keeps_plane_disabled_after_input_visibility_sync() {
    let mut cursor = test_cursor();
    cursor.mark_capability_quarantined();
    cursor.set_visible(true);

    assert!(!cursor.desired().visible);
    assert!(!cursor.needs_submission());
}

#[test]
fn initial_software_modeset_records_a_disabled_cursor_plane() {
    let mut cursor = test_cursor();
    cursor.desired.visible = true;

    cursor.mark_initial_submitted(None);

    assert!(!cursor.current().visible);
    assert_eq!(cursor.current().framebuffer_id, None);
    assert!(!cursor.needs_submission_for(None));
}

#[test]
fn cursor_plane_lifecycle_is_generation_scoped() {
    let mut lifecycle = CursorPlaneLifecycle::new(4);
    assert!(lifecycle.initial_clear_required());
    assert!(!lifecycle.confirm_initial_clear(3));
    assert!(lifecycle.initial_clear_required());
    assert!(lifecycle.confirm_initial_clear(4));
    assert!(!lifecycle.initial_clear_required());
    assert!(!lifecycle.confirm_initial_clear(4));

    assert!(lifecycle.rearm_generation(5));
    assert_eq!(lifecycle.generation(), 5);
    assert!(lifecycle.initial_clear_required());
}

#[test]
fn proven_cursor_capability_survives_motion_within_geometry_class_only() {
    let mut cursor = test_cursor();
    cursor.set_hardware_path_active(true);
    cursor.set_visible(true);
    let frozen = cursor.desired().clone();
    let frozen_key = cursor.capability_key_for(&frozen).unwrap();
    cursor.begin_submission_at_revision_with_capability_key(
        PageFlipToken::new(500).unwrap(),
        frozen,
        cursor.desired_epoch(),
        cursor.desired_revision(),
        Some(frozen_key),
    );
    assert!(cursor.current_capability_proven());

    cursor.set_position(100, 100);
    assert!(cursor.current_capability_proven());

    cursor.set_position(-1, 100);
    assert!(!cursor.current_capability_proven());
}

#[test]
fn capability_result_updates_the_frozen_key_after_desired_state_changes() {
    let mut cursor = test_cursor();
    let mut frozen = cursor.desired().clone();
    frozen.visible = true;
    frozen.x = 0;
    frozen.y = 100;
    frozen.framebuffer_id = Some(91);
    let frozen_key = cursor
        .capability_key_for(&frozen)
        .expect("the frozen cursor geometry is visible");

    let mut newer = frozen.clone();
    newer.x = 1919;
    cursor.desired = newer.clone();
    let newer_key = cursor
        .capability_key_for(&newer)
        .expect("the newer cursor geometry is visible");
    assert_ne!(frozen_key, newer_key);

    cursor.mark_capability_proven(frozen_key);

    assert_eq!(
        cursor.capability_status(frozen_key),
        CursorCapabilityStatus::Proven
    );
    assert_eq!(
        cursor.capability_status(newer_key),
        CursorCapabilityStatus::Unknown
    );
}

#[test]
fn capability_keys_cover_payload_identity_and_each_result_is_owner_bound() {
    let mut cursor = test_cursor();
    let mut state_a = cursor.desired().clone();
    state_a.visible = true;
    state_a.x = 0;
    state_a.y = 100;
    state_a.framebuffer_id = Some(91);
    let key_a = cursor.capability_key_for(&state_a).unwrap();

    let mut state_b = state_a.clone();
    state_b.x = 1_900;
    state_b.width = 32;
    state_b.height = 32;
    cursor.output_transform = 1;
    cursor.output_scale_milli = 1_250;
    cursor.plane.format_modifier.modifier = 9;
    let key_b = cursor.capability_key_for(&state_b).unwrap();
    assert_ne!(key_a, key_b);
    assert_ne!(key_a.destination_x, key_b.destination_x);
    assert_ne!(key_a.output_transform, key_b.output_transform);
    assert_ne!(key_a.output_scale_milli, key_b.output_scale_milli);
    assert_ne!(key_a.modifier, key_b.modifier);
    assert_ne!(key_a.cursor_width, key_b.cursor_width);

    // A successful TEST_ONLY/submit result for frozen A never identifies B.
    cursor.mark_capability_proven(key_a);
    assert_eq!(
        cursor.capability_status(key_a),
        CursorCapabilityStatus::Proven
    );
    assert_eq!(
        cursor.capability_status(key_b),
        CursorCapabilityStatus::Unknown
    );

    // Rejections are equally exact-key scoped.
    cursor.note_test_failure_for(Some(key_a));
    assert!(matches!(
        cursor.capability_status(key_a),
        CursorCapabilityStatus::Quarantined { .. }
    ));
    assert_eq!(
        cursor.capability_status(key_b),
        CursorCapabilityStatus::Unknown
    );
    cursor.note_submit_failure_for(Some(key_a));
    assert!(matches!(
        cursor.capability_status(key_a),
        CursorCapabilityStatus::Quarantined { .. }
    ));

    // EBUSY is represented by leaving A unknown until the retry succeeds.
    let mut retry = test_cursor();
    let retry_key = retry.capability_key_for(&state_a).unwrap();
    assert_eq!(
        retry.capability_status(retry_key),
        CursorCapabilityStatus::Unknown
    );
    retry.mark_capability_proven(retry_key);
    assert_eq!(
        retry.capability_status(retry_key),
        CursorCapabilityStatus::Proven
    );

    // A generation change invalidates the old frozen proof rather than
    // transferring it to the new-generation desired state.
    let mut recovered = test_cursor();
    let old_key = recovered.capability_key_for(&state_a).unwrap();
    recovered.mark_capability_proven(old_key);
    recovered.rearm_generation(2);
    assert_eq!(
        recovered.capability_status(old_key),
        CursorCapabilityStatus::Unknown
    );
}
