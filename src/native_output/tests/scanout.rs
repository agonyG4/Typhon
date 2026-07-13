use super::*;
use oblivion_one::native::kms::DrmFormatModifierPair;
use oblivion_one::render_backend::{
    buffer::{DrmFormat, DrmModifier},
    egl_gles::EglGlesDmabufFormat,
};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::{cell::RefCell, rc::Rc};

struct TestGbmAllocationProbe {
    rejected: Vec<DrmFormatModifierPair>,
}

impl GbmAllocationProbe for TestGbmAllocationProbe {
    fn supports(&mut self, candidate: DrmFormatModifierPair) -> bool {
        !self.rejected.contains(&candidate)
    }
}

#[test]
fn explicit_output_modifier_intersection_uses_exact_preference_order() {
    let xrgb_linear = DrmFormatModifierPair {
        fourcc: DrmFormat::XRGB8888_FOURCC,
        modifier: DrmModifier::LINEAR.0,
    };
    let argb_tiled = DrmFormatModifierPair {
        fourcc: DrmFormat::ARGB8888_FOURCC,
        modifier: 9,
    };
    let xrgb_tiled = DrmFormatModifierPair {
        fourcc: DrmFormat::XRGB8888_FOURCC,
        modifier: 11,
    };
    let drm = [xrgb_linear, argb_tiled, xrgb_tiled];
    let egl = [
        EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier::LINEAR),
        EglGlesDmabufFormat::new(DrmFormat::Argb8888, DrmModifier(9)),
        EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier(11)),
    ];

    let selected =
        select_output_format_modifier(&drm, &egl, &mut TestGbmAllocationProbe { rejected: vec![] })
            .unwrap();

    assert_eq!(selected, xrgb_tiled);
}

#[test]
fn explicit_output_modifier_intersection_probes_gbm_and_rejects_invalid_modifier() {
    let xrgb_tiled = DrmFormatModifierPair {
        fourcc: DrmFormat::XRGB8888_FOURCC,
        modifier: 11,
    };
    let argb_linear = DrmFormatModifierPair {
        fourcc: DrmFormat::ARGB8888_FOURCC,
        modifier: DrmModifier::LINEAR.0,
    };
    let invalid = DrmFormatModifierPair {
        fourcc: DrmFormat::XRGB8888_FOURCC,
        modifier: DrmModifier::INVALID.0,
    };
    let drm = [invalid, xrgb_tiled, argb_linear];
    let egl = [
        EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier::INVALID),
        EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier(11)),
        EglGlesDmabufFormat::new(DrmFormat::Argb8888, DrmModifier::LINEAR),
    ];

    let selected = select_output_format_modifier(
        &drm,
        &egl,
        &mut TestGbmAllocationProbe {
            rejected: vec![xrgb_tiled],
        },
    )
    .unwrap();

    assert_eq!(selected, argb_linear);
}

#[test]
fn explicit_addfb2_metadata_preserves_multiple_planes_and_modifier_flag() {
    let descriptor = ExplicitFramebufferDescriptor::new(
        1920,
        1080,
        DrmFormat::XRGB8888_FOURCC,
        &[
            ExplicitFramebufferPlane {
                handle: 10,
                pitch: 7680,
                offset: 0,
                modifier: 9,
            },
            ExplicitFramebufferPlane {
                handle: 11,
                pitch: 3840,
                offset: 8_294_400,
                modifier: 9,
            },
        ],
    )
    .unwrap();

    assert_eq!(descriptor.plane_count(), 2);
    assert_eq!(descriptor.handles()[..2], [10, 11]);
    assert_eq!(descriptor.pitches()[..2], [7680, 3840]);
    assert_eq!(descriptor.modifiers()[..2], [9, 9]);
    assert_eq!(descriptor.flags(), drm_sys::DRM_MODE_FB_MODIFIERS);
}

#[test]
fn explicit_addfb2_metadata_rejects_invalid_plane_count() {
    assert!(ExplicitFramebufferDescriptor::new(1, 1, DrmFormat::XRGB8888_FOURCC, &[]).is_err());
    assert!(
        ExplicitFramebufferDescriptor::new(
            1,
            1,
            DrmFormat::XRGB8888_FOURCC,
            &[ExplicitFramebufferPlane::default(); 5],
        )
        .is_err()
    );
}

#[derive(Default)]
struct TestFramebufferRegistration {
    added: Vec<u32>,
    removed: Vec<u32>,
    fail_at: usize,
}

impl ExplicitFramebufferRegistration for TestFramebufferRegistration {
    fn add(&mut self, _descriptor: &ExplicitFramebufferDescriptor) -> io::Result<u32> {
        if self.added.len() == self.fail_at {
            return Err(io::Error::other("injected AddFB2 failure"));
        }
        let id = 100 + u32::try_from(self.added.len()).unwrap();
        self.added.push(id);
        Ok(id)
    }

    fn remove(&mut self, framebuffer: u32) {
        self.removed.push(framebuffer);
    }
}

#[test]
fn explicit_addfb2_partial_creation_cleans_up_created_framebuffers() {
    let plane = ExplicitFramebufferPlane {
        handle: 10,
        pitch: 4,
        offset: 0,
        modifier: DrmModifier::LINEAR.0,
    };
    let descriptors = [
        ExplicitFramebufferDescriptor::new(1, 1, DrmFormat::XRGB8888_FOURCC, &[plane]).unwrap(),
        ExplicitFramebufferDescriptor::new(1, 1, DrmFormat::XRGB8888_FOURCC, &[plane]).unwrap(),
        ExplicitFramebufferDescriptor::new(1, 1, DrmFormat::XRGB8888_FOURCC, &[plane]).unwrap(),
    ];
    let mut registration = TestFramebufferRegistration {
        fail_at: 2,
        ..Default::default()
    };

    assert!(register_explicit_framebuffers(&mut registration, &descriptors).is_err());
    assert_eq!(registration.added, vec![100, 101]);
    assert_eq!(registration.removed, vec![101, 100]);
}

struct DropOrderProbe {
    id: u8,
    log: Rc<RefCell<Vec<String>>>,
}

impl Drop for DropOrderProbe {
    fn drop(&mut self) {
        self.log.borrow_mut().push(format!("bo{}", self.id));
    }
}

#[test]
fn output_slot_two_construction_failure_tears_down_resources_in_safe_order() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let slots = vec![
        DropOrderProbe {
            id: 0,
            log: Rc::clone(&log),
        },
        DropOrderProbe {
            id: 1,
            log: Rc::clone(&log),
        },
    ];

    teardown_slot_resources(
        &slots,
        |slot| log.borrow_mut().push(format!("gl{}", slot.id)),
        |slot| log.borrow_mut().push(format!("image{}", slot.id)),
        |slot| log.borrow_mut().push(format!("fb{}", slot.id)),
    );
    drop(slots);

    assert_eq!(
        *log.borrow(),
        ["gl0", "gl1", "image0", "image1", "fb0", "fb1", "bo0", "bo1"]
    );
}

#[test]
fn explicit_output_pool_has_exactly_three_slots() {
    let slots = OutputSlotSet::new([
        OutputSlotId::new(0).unwrap(),
        OutputSlotId::new(1).unwrap(),
        OutputSlotId::new(2).unwrap(),
    ])
    .unwrap();

    assert_eq!(slots.capacity(), 3);
}

#[test]
fn explicit_output_swapchain_requires_a_presented_current_slot() {
    let slots = OutputSlotSet::new([
        OutputSlotId::new(0).unwrap(),
        OutputSlotId::new(1).unwrap(),
        OutputSlotId::new(2).unwrap(),
    ])
    .unwrap();

    assert!(OutputSlotOwnership::from_presented_slots(slots, None).is_err());
    assert!(
        OutputSlotOwnership::from_presented_slots(slots, Some(OutputSlotId::new(0).unwrap()))
            .is_ok()
    );
}

#[test]
fn current_pending_ready_slots_never_alias() {
    let slots = OutputSlotSet::new([
        OutputSlotId::new(0).unwrap(),
        OutputSlotId::new(1).unwrap(),
        OutputSlotId::new(2).unwrap(),
    ])
    .unwrap();
    let mut ownership =
        OutputSlotOwnership::from_presented_slots(slots, Some(OutputSlotId::new(0).unwrap()))
            .unwrap();

    assert!(ownership.set_pending(OutputSlotId::new(1).unwrap()).is_ok());
    assert!(ownership.set_ready(OutputSlotId::new(1).unwrap()).is_err());
    assert!(ownership.set_ready(OutputSlotId::new(2).unwrap()).is_ok());
    assert!(ownership.set_ready(OutputSlotId::new(0).unwrap()).is_err());
}

fn explicit_slot_set() -> OutputSlotSet {
    OutputSlotSet::new([
        OutputSlotId::new(0).unwrap(),
        OutputSlotId::new(1).unwrap(),
        OutputSlotId::new(2).unwrap(),
    ])
    .unwrap()
}

fn test_sync_fd() -> OwnedFd {
    let mut pipe = [-1; 2];
    assert_eq!(
        unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
        0
    );
    unsafe { libc::close(pipe[1]) };
    unsafe { OwnedFd::from_raw_fd(pipe[0]) }
}

fn test_render_fence() -> crate::egl_renderer::native_fence::NativeRenderFence {
    crate::egl_renderer::native_fence::NativeRenderFence::from_submission_fd(test_sync_fd())
}

fn controllable_render_fence() -> (
    crate::egl_renderer::native_fence::NativeRenderFence,
    OwnedFd,
) {
    let event = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    assert!(event >= 0);
    let signal = unsafe { OwnedFd::from_raw_fd(event) };
    let submission = unsafe { libc::fcntl(signal.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    assert!(submission >= 0);
    (
        crate::egl_renderer::native_fence::NativeRenderFence::from_submission_fd(unsafe {
            OwnedFd::from_raw_fd(submission)
        }),
        signal,
    )
}

#[test]
fn explicit_output_swapchain_valid_current_pending_ready_transition() {
    let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
        explicit_slot_set(),
        OutputSlotId::new(0).unwrap(),
        7,
    )
    .unwrap();
    let slot_one = swapchain.acquire_render_slot().unwrap();
    assert_eq!(slot_one, OutputSlotId::new(1).unwrap());
    swapchain
        .finish_render(slot_one, 10, test_render_fence())
        .unwrap();
    let token_one = PageFlipToken::new(101).unwrap();
    swapchain.submit_ready(token_one, None).unwrap();

    let slot_two = swapchain.acquire_render_slot().unwrap();
    assert_eq!(slot_two, OutputSlotId::new(2).unwrap());
    swapchain
        .finish_render(slot_two, 11, test_render_fence())
        .unwrap();
    assert_eq!(swapchain.pending_slot(), Some(slot_one));
    assert_eq!(swapchain.ready_slot(), Some(slot_two));

    let completed = swapchain.complete_pageflip(token_one, 7).unwrap();
    assert_eq!(completed.old_current, OutputSlotId::new(0).unwrap());
    assert_eq!(completed.new_current, slot_one);
    assert_eq!(swapchain.current(), slot_one);
    assert_eq!(swapchain.ready_slot(), Some(slot_two));
    swapchain
        .submit_ready(PageFlipToken::new(102).unwrap(), None)
        .unwrap();
    assert_eq!(swapchain.pending_slot(), Some(slot_two));
    swapchain.validate_invariants().unwrap();
}

#[test]
fn explicit_output_swapchain_rejects_invalid_transitions_without_mutation() {
    let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
        explicit_slot_set(),
        OutputSlotId::new(0).unwrap(),
        9,
    )
    .unwrap();
    assert!(
        swapchain
            .submit_ready(PageFlipToken::new(1).unwrap(), None)
            .is_err()
    );
    let rendering = swapchain.acquire_render_slot().unwrap();
    assert!(swapchain.acquire_render_slot().is_err());
    assert!(
        swapchain
            .finish_render(OutputSlotId::new(2).unwrap(), 1, test_render_fence())
            .is_err()
    );
    swapchain
        .finish_render(rendering, 1, test_render_fence())
        .unwrap();
    assert!(swapchain.acquire_render_slot().is_err());
    let token = PageFlipToken::new(10).unwrap();
    swapchain.submit_ready(token, None).unwrap();
    assert!(
        swapchain
            .submit_ready(PageFlipToken::new(11).unwrap(), None)
            .is_err()
    );
    assert!(
        swapchain
            .complete_pageflip(PageFlipToken::new(11).unwrap(), 9)
            .is_err()
    );
    assert_eq!(swapchain.pending_slot(), Some(rendering));
    assert!(swapchain.complete_pageflip(token, 8).is_err());
    assert_eq!(swapchain.pending_slot(), Some(rendering));
    swapchain.validate_invariants().unwrap();
}

#[test]
fn atomic_submit_failure_quarantines_ready_slot_and_poison_rejects_operations() {
    let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
        explicit_slot_set(),
        OutputSlotId::new(0).unwrap(),
        4,
    )
    .unwrap();
    let slot = swapchain.acquire_render_slot().unwrap();
    swapchain
        .finish_render(slot, 1, test_render_fence())
        .unwrap();

    assert_eq!(swapchain.atomic_submit_failed().unwrap(), slot);
    assert_eq!(swapchain.quarantine_slot_id(), Some(slot));
    assert!(swapchain.is_poisoned());
    assert!(swapchain.acquire_render_slot().is_err());
    assert!(
        swapchain
            .submit_ready(PageFlipToken::new(1).unwrap(), None)
            .is_err()
    );
    swapchain.validate_invariants().unwrap();
}

#[test]
fn post_draw_failure_quarantines_rendering_slot_without_freeing_it() {
    let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
        explicit_slot_set(),
        OutputSlotId::new(0).unwrap(),
        4,
    )
    .unwrap();
    let slot = swapchain.acquire_render_slot().unwrap();

    assert_eq!(
        swapchain
            .quarantine_rendering(None, OutputQuarantineReason::RenderFenceExportFailure)
            .unwrap(),
        slot
    );
    assert_eq!(swapchain.rendering_slot(), None);
    assert_eq!(swapchain.quarantine_slot_id(), Some(slot));
    assert!(swapchain.acquire_render_slot().is_err());
    swapchain.validate_invariants().unwrap();
}

#[test]
fn suspended_ready_slot_cannot_be_reused_before_fence_proof() {
    let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
        explicit_slot_set(),
        OutputSlotId::new(0).unwrap(),
        12,
    )
    .unwrap();
    let slot = swapchain.acquire_render_slot().unwrap();
    swapchain
        .finish_render(slot, 1, test_render_fence())
        .unwrap();
    let abandoned = swapchain
        .suspend_abandon_ready()
        .unwrap()
        .expect("ready frame should be returned for protocol disposition");
    assert_eq!(abandoned.slot, slot);

    assert!(!swapchain.is_poisoned());
    assert!(swapchain.acquire_render_slot().is_err());
    assert!(swapchain.recover_suspended_slot(false).is_err());
    assert_eq!(swapchain.quarantine_slot_id(), Some(slot));
    swapchain.recover_suspended_slot(true).unwrap();
    assert_eq!(swapchain.quarantine_slot_id(), None);
    assert_eq!(swapchain.acquire_render_slot().unwrap(), slot);
    swapchain.validate_invariants().unwrap();
}

#[test]
fn recovery_retires_unpresented_pending_frame_without_promoting_it() {
    let current = OutputSlotId::new(0).unwrap();
    let mut swapchain =
        AtomicOutputSwapchain::from_presented_slots(explicit_slot_set(), current, 13).unwrap();
    let pending = swapchain.acquire_render_slot().unwrap();
    swapchain
        .finish_render(pending, 1, test_render_fence())
        .unwrap();
    swapchain
        .submit_ready(PageFlipToken::new(7).unwrap(), None)
        .unwrap();

    let retired = swapchain
        .retire_pending_after_recovery()
        .expect("pending frame should be returned for protocol disposition");
    assert_eq!(retired.slot, pending);
    assert_eq!(swapchain.current(), current);
    assert_eq!(swapchain.pending_slot(), None);
    assert_eq!(swapchain.acquire_render_slot().unwrap(), pending);
    swapchain.validate_invariants().unwrap();
}

#[test]
fn suspended_ready_fence_requires_an_observed_signal_before_recovery() {
    let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
        explicit_slot_set(),
        OutputSlotId::new(0).unwrap(),
        14,
    )
    .unwrap();
    let slot = swapchain.acquire_render_slot().unwrap();
    let (fence, signal) = controllable_render_fence();
    swapchain.finish_render(slot, 1, fence).unwrap();
    let _abandoned = swapchain.suspend_abandon_ready().unwrap().unwrap();

    assert!(!swapchain.suspended_ready_fence_signaled().unwrap());
    let value = 1u64.to_ne_bytes();
    assert_eq!(
        unsafe { libc::write(signal.as_raw_fd(), value.as_ptr().cast(), value.len(),) },
        value.len() as isize
    );
    assert!(swapchain.suspended_ready_fence_signaled().unwrap());
    swapchain.recover_suspended_slot(true).unwrap();
    assert_eq!(swapchain.acquire_render_slot().unwrap(), slot);
}

#[test]
fn pool_generation_changes_only_after_recovery_ownership_is_retired() {
    let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
        explicit_slot_set(),
        OutputSlotId::new(0).unwrap(),
        20,
    )
    .unwrap();
    let slot = swapchain.acquire_render_slot().unwrap();
    swapchain
        .finish_render(slot, 1, test_render_fence())
        .unwrap();
    assert!(swapchain.rebind_pool_generation(21).is_err());
    let _abandoned = swapchain.suspend_abandon_ready().unwrap().unwrap();
    swapchain.recover_suspended_slot(true).unwrap();

    swapchain.rebind_pool_generation(21).unwrap();
    assert_eq!(swapchain.pool_generation(), 21);
    swapchain.validate_invariants().unwrap();
}

#[test]
fn proc_stat_cpu_parser_reads_user_and_system_ticks_after_comm() {
    let stat = "1234 (oblivion one) S 1 2 3 4 5 6 7 8 9 10 123 45 0 0 20 0";

    assert_eq!(
        parse_proc_stat_cpu_ticks(stat),
        Some(NativeProcessCpuSample {
            user_ticks: 123,
            system_ticks: 45,
        })
    );
}

#[test]
fn kms_mode_preference_parses_exact_resolution_and_refresh() {
    assert_eq!(
        NativeModePreference::parse("1920x1080@165"),
        NativeModePreference::Exact {
            width: 1920,
            height: 1080,
            refresh_hz: Some(165),
        }
    );
}

#[test]
fn kms_mode_selection_prefers_exact_refresh_when_available() {
    let modes = [
        test_drm_mode(1920, 1080, 60),
        test_drm_mode(2560, 1440, 144),
        test_drm_mode(1920, 1080, 165),
    ];

    let selected = select_kms_mode(
        &modes,
        NativeModePreference::Exact {
            width: 1920,
            height: 1080,
            refresh_hz: Some(165),
        },
    )
    .expect("exact mode should be selected");

    assert_eq!(mode_tuple(&selected), (1920, 1080, 165));
}

#[test]
fn kms_mode_selection_uses_nearest_refresh_for_exact_resolution() {
    let modes = [
        test_drm_mode(1920, 1080, 60),
        test_drm_mode(1920, 1080, 144),
        test_drm_mode(2560, 1440, 75),
    ];

    let selected = select_kms_mode(
        &modes,
        NativeModePreference::Exact {
            width: 1920,
            height: 1080,
            refresh_hz: Some(165),
        },
    )
    .expect("nearest refresh should be selected");

    assert_eq!(mode_tuple(&selected), (1920, 1080, 144));
}

#[test]
fn kms_mode_selection_highrr_prioritizes_refresh_then_resolution() {
    let modes = [
        test_drm_mode(3840, 2160, 60),
        test_drm_mode(2560, 1440, 144),
        test_drm_mode(1920, 1080, 165),
    ];

    let selected =
        select_kms_mode(&modes, NativeModePreference::HighRefresh).expect("mode selected");

    assert_eq!(mode_tuple(&selected), (1920, 1080, 165));
}

#[test]
fn kms_mode_selection_auto_prioritizes_resolution_then_refresh() {
    let modes = [
        test_drm_mode(1920, 1080, 165),
        test_drm_mode(2560, 1440, 60),
        test_drm_mode(2560, 1440, 144),
    ];

    let selected = select_kms_mode(&modes, NativeModePreference::Auto).expect("mode selected");

    assert_eq!(mode_tuple(&selected), (2560, 1440, 144));
}

#[test]
fn select_crtc_prefers_encoder_current_crtc_when_available() {
    let encoder = drm_sys::drm_mode_get_encoder {
        crtc_id: 42,
        possible_crtcs: 0b010,
        ..Default::default()
    };

    assert_eq!(select_crtc_id(&[12, 42, 77], &encoder), Some(42));
}

#[test]
fn select_crtc_falls_back_to_possible_crtc_bitset() {
    let encoder = drm_sys::drm_mode_get_encoder {
        crtc_id: 0,
        possible_crtcs: 0b100,
        ..Default::default()
    };

    assert_eq!(select_crtc_id(&[12, 42, 77], &encoder), Some(77));
}

fn test_drm_mode(width: u16, height: u16, refresh_hz: u32) -> drm_sys::drm_mode_modeinfo {
    drm_sys::drm_mode_modeinfo {
        hdisplay: width,
        vdisplay: height,
        vrefresh: refresh_hz,
        ..Default::default()
    }
}

fn mode_tuple(mode: &drm_sys::drm_mode_modeinfo) -> (u16, u16, u32) {
    (mode.hdisplay, mode.vdisplay, mode.vrefresh)
}

#[test]
fn native_drm_backend_plan_prefers_libseat_when_available() {
    let plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
        preference: NativeDrmBackendPreference::Auto,
        seat_available: true,
    });

    assert_eq!(plan.primary, NativeDrmBackendKind::Libseat);
    assert!(plan.fallbacks.is_empty());
}

#[test]
fn native_drm_backend_plan_uses_direct_without_seat() {
    let plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
        preference: NativeDrmBackendPreference::Auto,
        seat_available: false,
    });

    assert_eq!(plan.primary, NativeDrmBackendKind::Direct);
    assert!(plan.fallbacks.is_empty());
}

#[test]
fn native_drm_backend_plan_can_force_libseat() {
    let plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
        preference: NativeDrmBackendPreference::Libseat,
        seat_available: true,
    });

    assert_eq!(plan.primary, NativeDrmBackendKind::Libseat);
    assert!(plan.fallbacks.is_empty());
}

#[test]
fn native_drm_backend_plan_rejects_forced_libseat_without_seat() {
    let plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
        preference: NativeDrmBackendPreference::Libseat,
        seat_available: false,
    });

    assert_eq!(plan.primary, NativeDrmBackendKind::Unavailable);
    assert!(plan.fallbacks.is_empty());
}

#[test]
fn native_scanout_plan_prefers_native_egl_gbm_when_ready() {
    let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
        preference: NativeScanoutPreference::Auto,
        gbm_available: true,
        egl_available: true,
        page_flip_available: true,
    });

    assert_eq!(plan.primary, NativeScanoutKind::AtomicEglGbmExplicit);
    assert_eq!(
        plan.fallbacks,
        vec![
            NativeScanoutKind::GbmCpuWritePageFlip,
            NativeScanoutKind::DumbFramebuffer
        ]
    );
}

#[test]
fn native_scanout_parser_keeps_opaque_egl_gbm_explicit_only() {
    assert_eq!(
        NativeScanoutPreference::parse("native-egl-gbm-opaque"),
        NativeScanoutPreference::NativeEglGbmOpaqueCompatibility
    );
    assert_eq!(
        NativeScanoutPreference::parse("native-egl-gbm"),
        NativeScanoutPreference::AtomicEglGbmExplicit
    );
    assert_eq!(
        NativeScanoutPreference::parse("gpu"),
        NativeScanoutPreference::AtomicEglGbmExplicit
    );
    assert_eq!(
        NativeScanoutKind::AtomicEglGbmExplicit.metric_name(),
        "atomic-egl-gbm-explicit"
    );
    assert_eq!(
        NativeScanoutKind::NativeEglGbmOpaqueCompatibility.metric_name(),
        "native-egl-gbm-opaque"
    );
}

#[test]
fn native_scanout_plan_can_force_gpu_without_cpu_fallback() {
    let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
        preference: NativeScanoutPreference::AtomicEglGbmExplicit,
        gbm_available: true,
        egl_available: true,
        page_flip_available: true,
    });

    assert_eq!(plan.primary, NativeScanoutKind::AtomicEglGbmExplicit);
    assert!(plan.fallbacks.is_empty());
}

#[test]
fn native_scanout_plan_rejects_forced_gpu_without_egl() {
    let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
        preference: NativeScanoutPreference::AtomicEglGbmExplicit,
        gbm_available: true,
        egl_available: false,
        page_flip_available: true,
    });

    assert_eq!(plan.primary, NativeScanoutKind::Unavailable);
    assert!(plan.fallbacks.is_empty());
}

#[test]
fn native_scanout_plan_fallback_after_gpu_failure_preserves_remaining_candidates() {
    let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
        preference: NativeScanoutPreference::Auto,
        gbm_available: true,
        egl_available: true,
        page_flip_available: true,
    })
    .after_failed(NativeScanoutKind::AtomicEglGbmExplicit);

    assert_eq!(plan.primary, NativeScanoutKind::GbmCpuWritePageFlip);
    assert_eq!(plan.fallbacks, vec![NativeScanoutKind::DumbFramebuffer]);
}

#[test]
fn native_scanout_kind_names_cpu_write_gbm_backend_honestly() {
    assert_eq!(
        NativeScanoutKind::GbmCpuWritePageFlip.as_str(),
        "GBM CPU-write pageflip"
    );
}

#[test]
fn injected_native_egl_gbm_open_failure_returns_clear_error_before_kms_use() {
    let previous = std::env::var_os("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM");
    unsafe {
        std::env::set_var("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM", "1");
    }
    let file = fs::File::open("Cargo.toml").unwrap();

    let error = match NativeScanoutBackend::open_kind(
        NativeScanoutKind::NativeEglGbmOpaqueCompatibility,
        &file,
        1,
        1,
        1,
    ) {
        Ok(_) => panic!("injected native EGL/GBM failure should fail before KMS use"),
        Err(error) => error,
    };

    match previous {
        Some(value) => unsafe {
            std::env::set_var("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM", value);
        },
        None => unsafe {
            std::env::remove_var("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM");
        },
    }
    assert!(
        error
            .to_string()
            .contains("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM")
    );
}

#[test]
fn auto_gpu_open_failure_next_cpu_candidate_resolves_apps_to_cpu() {
    let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
        preference: NativeScanoutPreference::Auto,
        gbm_available: true,
        egl_available: true,
        page_flip_available: true,
    });

    let fallback = plan.after_failed(NativeScanoutKind::AtomicEglGbmExplicit);

    assert_eq!(fallback.primary, NativeScanoutKind::GbmCpuWritePageFlip);
    assert_eq!(
        resolve_native_app_gpu_policy(CompositorAppGpuPreference::Auto, fallback.primary).unwrap(),
        EffectiveCompositorAppGpuPolicy::CpuOnly
    );
}

#[test]
fn native_scanout_preference_keeps_legacy_gbm_egl_alias_for_cpu_write_backend() {
    assert_eq!(
        NativeScanoutPreference::parse("gbm-egl"),
        NativeScanoutPreference::GbmCpuWritePageFlip
    );
}

#[test]
fn native_scanout_preference_accepts_canonical_cpu_write_backend_name() {
    assert_eq!(
        NativeScanoutPreference::parse("gbm-cpu-write"),
        NativeScanoutPreference::GbmCpuWritePageFlip
    );
}

#[test]
fn native_scanout_preference_accepts_canonical_gpu_backend_name() {
    assert_eq!(
        NativeScanoutPreference::parse("native-egl-gbm"),
        NativeScanoutPreference::AtomicEglGbmExplicit
    );
    assert_eq!(
        NativeScanoutPreference::parse("gpu"),
        NativeScanoutPreference::AtomicEglGbmExplicit
    );
}

#[test]
fn native_scanout_plan_uses_cpu_gbm_fallback_without_egl() {
    let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
        preference: NativeScanoutPreference::Auto,
        gbm_available: true,
        egl_available: false,
        page_flip_available: true,
    });

    assert_eq!(plan.primary, NativeScanoutKind::GbmCpuWritePageFlip);
    assert_eq!(plan.fallbacks, vec![NativeScanoutKind::DumbFramebuffer]);
}

#[test]
fn native_pageflip_state_blocks_overlapping_flips() {
    let mut state = AtomicCommitState::default();
    let token = PageFlipToken::new(allocate_native_page_flip_token()).unwrap();
    let framebuffer = FramebufferId::new(17).unwrap();

    assert!(!state.is_pending());
    state.begin(token, framebuffer, 3, Instant::now()).unwrap();
    assert!(state.is_pending());
    assert!(
        state
            .begin(
                PageFlipToken::new(allocate_native_page_flip_token()).unwrap(),
                FramebufferId::new(18).unwrap(),
                3,
                Instant::now(),
            )
            .is_err()
    );
    assert_eq!(
        state.complete(token, 3),
        AtomicCompletion::Completed { framebuffer }
    );
    assert!(!state.is_pending());
    assert_eq!(state.complete(token, 3), AtomicCompletion::Stale);
}

#[test]
fn native_pageflip_state_rejects_mismatch_without_clearing_pending() {
    let mut state = AtomicCommitState::default();
    let expected = PageFlipToken::new(allocate_native_page_flip_token()).unwrap();
    let received = PageFlipToken::new(next_nonzero_page_flip_token(expected.get())).unwrap();
    state
        .begin(expected, FramebufferId::new(21).unwrap(), 5, Instant::now())
        .unwrap();

    assert_eq!(state.complete(received, 5), AtomicCompletion::Mismatched);
    assert_eq!(state.pending_token(), Some(expected));
}

#[test]
fn native_pageflip_state_stale_event_cannot_complete_new_submission() {
    let mut state = AtomicCommitState::default();
    let first = PageFlipToken::new(allocate_native_page_flip_token()).unwrap();
    let first_framebuffer = FramebufferId::new(31).unwrap();
    state
        .begin(first, first_framebuffer, 7, Instant::now())
        .unwrap();
    assert_eq!(
        state.complete(first, 7),
        AtomicCompletion::Completed {
            framebuffer: first_framebuffer
        }
    );
    let second = PageFlipToken::new(allocate_native_page_flip_token()).unwrap();
    state
        .begin(second, FramebufferId::new(32).unwrap(), 7, Instant::now())
        .unwrap();

    assert_eq!(state.complete(first, 7), AtomicCompletion::Mismatched);
    assert_eq!(state.pending_token(), Some(second));
}

#[test]
fn native_pageflip_token_wrap_skips_zero() {
    assert_eq!(next_nonzero_page_flip_token(u64::MAX), 1);
    assert_eq!(next_nonzero_page_flip_token(1), 2);
}

#[test]
fn native_pageflip_token_does_not_restart_after_backend_recreation() {
    let mut first = AtomicCommitState::default();
    let old_token = PageFlipToken::new(allocate_native_page_flip_token()).unwrap();
    let framebuffer = FramebufferId::new(41).unwrap();
    first
        .begin(old_token, framebuffer, 11, Instant::now())
        .unwrap();
    assert_eq!(
        first.complete(old_token, 11),
        AtomicCompletion::Completed { framebuffer }
    );
    let mut replacement = AtomicCommitState::default();
    let replacement_token = PageFlipToken::new(allocate_native_page_flip_token()).unwrap();
    replacement
        .begin(
            replacement_token,
            FramebufferId::new(42).unwrap(),
            12,
            Instant::now(),
        )
        .unwrap();

    assert_ne!(replacement_token, old_token);
    assert_eq!(
        replacement.complete(old_token, 11),
        AtomicCompletion::StaleGeneration
    );
    assert_eq!(replacement.pending_token(), Some(replacement_token));
}

#[test]
fn native_pageflip_buffers_promote_ready_to_pending_to_current() {
    let mut buffers = NativePageFlipBuffers::default();

    buffers.set_ready(10);
    assert_eq!(buffers.ready_or_current(), Some(&10));
    assert_eq!(buffers.take_ready(), Some(10));
    buffers.set_pending(10);
    assert!(buffers.complete_page_flip());
    assert_eq!(buffers.ready_or_current(), Some(&10));

    assert!(!buffers.complete_page_flip());
}

#[test]
fn native_pageflip_buffers_finish_initial_scanout_promotes_ready() {
    let mut buffers = NativePageFlipBuffers::default();

    buffers.set_ready(20);
    buffers.finish_initial_scanout();

    assert_eq!(buffers.ready_or_current(), Some(&20));
    assert_eq!(buffers.take_ready(), None);
}

#[test]
fn suspended_cpu_gbm_pending_index_is_not_renderable_before_recovery() {
    assert_eq!(next_free_scanout_index(3, 0, None, Some(1)), Some(2));
}

#[test]
fn cpu_gbm_session_recovery_promotes_ready_index_and_keeps_it_out_of_render_targets() {
    let framebuffers = [10, 20, 30];
    let mut current_index = 0;
    let mut ready_index = Some(1);
    let mut pending_index = Some(2);
    let recovery = prepare_indexed_session_recovery(&framebuffers, current_index, ready_index)
        .expect("ready index should be selected for recovery");

    assert_eq!(recovery.framebuffer_id, 20);

    complete_indexed_session_recovery(
        &framebuffers,
        &mut current_index,
        &mut ready_index,
        &mut pending_index,
        recovery,
    )
    .expect("selected ready index should become current");

    assert_eq!(current_index, 1);
    assert_eq!(ready_index, None);
    assert_eq!(pending_index, None);
    assert_eq!(
        next_free_scanout_index(
            framebuffers.len(),
            current_index,
            ready_index,
            pending_index
        ),
        Some(0)
    );
}

#[test]
fn cpu_gbm_session_recovery_keeps_current_index_when_no_ready_index_exists() {
    let framebuffers = [10, 20, 30];
    let mut current_index = 0;
    let mut ready_index = None;
    let mut pending_index = Some(1);
    let recovery = prepare_indexed_session_recovery(&framebuffers, current_index, ready_index)
        .expect("current index should be selected for recovery");

    assert_eq!(recovery.framebuffer_id, 10);

    complete_indexed_session_recovery(
        &framebuffers,
        &mut current_index,
        &mut ready_index,
        &mut pending_index,
        recovery,
    )
    .expect("selected current index should remain current");

    assert_eq!(current_index, 0);
    assert_eq!(ready_index, None);
    assert_eq!(pending_index, None);
}

#[test]
fn cpu_gbm_session_recovery_rejects_a_different_ready_index_after_preparation() {
    let framebuffers = [10, 20, 30];
    let mut current_index = 0;
    let mut ready_index = Some(1);
    let mut pending_index = Some(2);
    let recovery = prepare_indexed_session_recovery(&framebuffers, current_index, ready_index)
        .expect("ready index should be selected for recovery");
    ready_index = Some(2);

    assert!(
        complete_indexed_session_recovery(
            &framebuffers,
            &mut current_index,
            &mut ready_index,
            &mut pending_index,
            recovery,
        )
        .is_err()
    );
    assert_eq!(current_index, 0);
    assert_eq!(ready_index, Some(2));
    assert_eq!(pending_index, Some(2));
}

#[test]
fn native_initial_frame_has_no_builtin_compositor_ui() {
    let mut renderer = NativeFrameRenderer::default();
    let frame = renderer
        .render_frame(NativeFrameRequest {
            width: 320,
            height: 200,
            surfaces: &[],
            external_overlay_surface_ids: Vec::new(),
            visual_state: DesktopVisualState::wallpaper_only(),
            render_generation: 0,
            client_cursor: None,
        })
        .pixels
        .to_vec();
    let mut wallpaper = vec![0; 320 * 200];
    compose_output(
        &mut wallpaper,
        320,
        200,
        &[],
        DesktopVisualState::wallpaper_only(),
    );

    assert_eq!(frame, wallpaper);
}
