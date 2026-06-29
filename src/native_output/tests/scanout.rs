use super::*;
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
    assert_eq!(plan.fallbacks, vec![NativeDrmBackendKind::Direct]);
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

    assert_eq!(plan.primary, NativeScanoutKind::NativeEglGbm);
    assert_eq!(
        plan.fallbacks,
        vec![
            NativeScanoutKind::GbmCpuWritePageFlip,
            NativeScanoutKind::DumbFramebuffer
        ]
    );
}

#[test]
fn native_scanout_plan_can_force_gpu_without_cpu_fallback() {
    let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
        preference: NativeScanoutPreference::NativeEglGbm,
        gbm_available: true,
        egl_available: true,
        page_flip_available: true,
    });

    assert_eq!(plan.primary, NativeScanoutKind::NativeEglGbm);
    assert!(plan.fallbacks.is_empty());
}

#[test]
fn native_scanout_plan_rejects_forced_gpu_without_egl() {
    let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
        preference: NativeScanoutPreference::NativeEglGbm,
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
    .after_failed(NativeScanoutKind::NativeEglGbm);

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

    let error =
        match NativeScanoutBackend::open_kind(NativeScanoutKind::NativeEglGbm, &file, 1, 1, 1) {
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

    let fallback = plan.after_failed(NativeScanoutKind::NativeEglGbm);

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
        NativeScanoutPreference::NativeEglGbm
    );
    assert_eq!(
        NativeScanoutPreference::parse("gpu"),
        NativeScanoutPreference::NativeEglGbm
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
fn native_initial_frame_paints_real_topbar() {
    let mut renderer = NativeFrameRenderer::default();
    let spotlight = SpotlightModel::default();
    let frame = renderer
        .render_frame(NativeFrameRequest {
            width: 320,
            height: 200,
            surfaces: &[],
            dock_items: Vec::new(),
            spotlight: &spotlight,
            shell_generation: 0,
            visual_state: DesktopVisualState::wallpaper_only(),
            render_generation: 0,
            client_cursor: None,
        })
        .pixels
        .to_vec();
    let mut wallpaper = vec![0; 320 * 200];
    compose_nested_output(
        &mut wallpaper,
        320,
        200,
        &[],
        DesktopVisualState::wallpaper_only(),
    );

    assert_ne!(frame[16 * 320 + 160], wallpaper[16 * 320 + 160]);
}
