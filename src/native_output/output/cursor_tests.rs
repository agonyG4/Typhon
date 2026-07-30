use super::*;
use crate::native_output::OutputTransactionId;
use crate::native_output::runtime::{
    NativeCursorOutputArbitration, update_cursor_output_arbitration,
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
        desired_epoch: INITIAL_CURSOR_EPOCH,
        submitted_epoch: INITIAL_CURSOR_EPOCH,
        revisions: CursorRevisionTracker::new(),
        hardware_path_active: false,
        dirty: AtomicCursorDirty::default(),
        counters: AtomicCursorCounters::default(),
        plane_lifecycle: CursorPlaneLifecycle::new(1),
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
    cursor.begin_submission_at_revision(
        token,
        queued.visual_state,
        queued.cursor_epoch,
        queued.revision,
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
    cursor.begin_submission_at_revision(
        token,
        queued.visual_state,
        queued.cursor_epoch,
        queued.revision,
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
fn idle_theme_cursor_motion_opens_cursor_only_deadline_without_scene_damage() {
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
    };
    let mut next = first;
    next.commit_sequence += 1;
    assert_ne!(first, next);
    next = first;
    next.hotspot_x += 1;
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
fn failure_latch_keeps_plane_disabled_after_input_visibility_sync() {
    let mut cursor = test_cursor();
    cursor.mark_failure_latched();
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
fn cursor_plane_lifecycle_is_generation_scoped_and_invalidates_quarantine() {
    let mut lifecycle = CursorPlaneLifecycle::new(4);
    assert!(lifecycle.initial_clear_required());
    assert_eq!(
        lifecycle.capability_status(),
        CursorCapabilityStatus::Unknown
    );

    lifecycle.quarantine(CursorQuarantineReason::UnsupportedSize);
    assert!(matches!(
        lifecycle.capability_status(),
        CursorCapabilityStatus::Quarantined {
            reason: CursorQuarantineReason::UnsupportedSize,
            failure_count: 1,
        }
    ));
    assert!(!lifecycle.confirm_initial_clear(3));
    assert!(lifecycle.initial_clear_required());
    assert!(lifecycle.confirm_initial_clear(4));
    assert!(!lifecycle.initial_clear_required());
    assert!(!lifecycle.confirm_initial_clear(4));

    assert!(lifecycle.rearm_generation(5));
    assert_eq!(lifecycle.generation(), 5);
    assert_eq!(
        lifecycle.capability_status(),
        CursorCapabilityStatus::Unknown
    );
    assert!(lifecycle.initial_clear_required());
}
