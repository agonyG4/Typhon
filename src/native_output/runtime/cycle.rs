use super::*;

pub fn run(
    server: OwnCompositorServer,
    app: Vec<String>,
    app_gpu_preference: CompositorAppGpuPreference,
) -> NativeResult<()> {
    NativeRuntime::bootstrap(NativeRuntimeConfig {
        server,
        app,
        app_gpu_preference,
    })?
    .run()
}

pub(crate) fn run_legacy_native_runtime(
    mut server: OwnCompositorServer,
    app: Vec<String>,
    app_gpu_preference: CompositorAppGpuPreference,
) -> NativeResult<()> {
    let perf = NativePerfLogger::from_env();
    let startup_app = (!app.is_empty()).then_some(app);
    let bootstrap = NativeOutputBootstrap::discover();
    let session_probe = NativeSessionProbe::detect();
    let vrr_preference = NativeVrrPreference::from_env();
    let vrr_plan = NativeVrrPlan::choose(
        vrr_preference,
        bootstrap
            .connector
            .as_ref()
            .and_then(|connector| connector.vrr_capable),
    );

    println!("Opening native output backend.");
    println!("Wayland socket active: {}", server.socket_name());
    println!(
        "runtime dir: {}",
        display_optional_path(bootstrap.runtime_dir.as_deref())
    );
    println!(
        "kms device: {}",
        display_optional_path(bootstrap.kms_device.as_deref())
    );
    println!(
        "render device: {}",
        display_optional_path(bootstrap.render_device.as_deref())
    );
    if let Some(connector) = bootstrap.connector.as_ref() {
        println!("connected output: {}", connector.name);
        println!(
            "output enabled: {}",
            connector.enabled.as_deref().unwrap_or("unknown")
        );
        println!(
            "preferred mode: {}",
            connector.preferred_mode().unwrap_or("unknown")
        );
        println!(
            "vrr capable: {}",
            connector
                .vrr_capable
                .map(|capable| if capable { "yes" } else { "no" })
                .unwrap_or("unknown")
        );
    } else {
        println!("connected output: missing");
    }
    println!(
        "native VRR target: {} (supported {}, planned {})",
        vrr_plan.requested.as_str(),
        if vrr_plan.supported { "yes" } else { "no" },
        if vrr_plan.planned_enabled {
            "yes"
        } else {
            "no"
        }
    );
    match bootstrap.kms_resources.as_ref() {
        Ok(Some(resources)) => {
            println!(
                "kms resources: {} crtc(s), {} connector(s), {} encoder(s)",
                resources.crtc_count, resources.connector_count, resources.encoder_count
            );
            println!(
                "kms connected connectors: {}",
                resources.connected_connector_count
            );
            println!(
                "kms first connected connector id: {}",
                resources
                    .first_connected_connector_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "missing".to_string())
            );
            println!(
                "kms first connected mode: {}",
                resources
                    .first_connected_mode
                    .as_deref()
                    .unwrap_or("missing")
            );
        }
        Ok(None) => println!("kms resources: missing"),
        Err(error) => println!("kms resources: unavailable ({error})"),
    }
    println!(
        "native session input target: {}",
        session_probe.plan.input_strategy.as_str()
    );
    println!(
        "native session output target: {}",
        session_probe.plan.output_strategy.as_str()
    );
    println!("native session backend: libseat-managed input/DRM + GBM pageflip fallback");
    perf.log("native.start", || {
        vec![
            NativePerfField::str("socket", server.socket_name()),
            NativePerfField::str(
                "kms_device",
                display_optional_path(bootstrap.kms_device.as_deref()),
            ),
            NativePerfField::str(
                "render_device",
                display_optional_path(bootstrap.render_device.as_deref()),
            ),
            NativePerfField::str("vrr_policy", vrr_plan.requested.as_str()),
            NativePerfField::bool("vrr_supported", vrr_plan.supported),
            NativePerfField::bool("vrr_planned", vrr_plan.planned_enabled),
            NativePerfField::str("input_target", session_probe.plan.input_strategy.as_str()),
            NativePerfField::str("output_target", session_probe.plan.output_strategy.as_str()),
        ]
    });
    for warning in session_probe.plan.warnings() {
        eprintln!("native session: {warning}");
    }
    if let Some(command) = startup_app.as_ref() {
        println!("startup app command deferred until native scanout is ready: {command:?}");
        perf.log("app.deferred", || {
            vec![
                NativePerfField::usize("argc", command.len()),
                NativePerfField::str("command", command.join(" ")),
            ]
        });
    }

    if host_display_variables_available() && !native_scanout_forced() {
        return Err(io::Error::other(
            "native scanout guarded because a host display is active; set OBLIVION_ONE_NATIVE_SCANOUT=1 to take over the DRM output",
        )
        .into());
    }

    let Some(kms_device) = bootstrap.kms_device.as_deref() else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no /dev/dri/card* KMS device found",
        )
        .into());
    };

    probe_native_egl_gbm_device(&bootstrap, &perf);

    let seat_session = open_native_seat_session(&session_probe);
    let drm_plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
        preference: NativeDrmBackendPreference::from_env(),
        seat_available: seat_session.is_some(),
    });
    println!("native DRM backend target: {}", drm_plan.primary.as_str());
    let kms = NativeDrmDevice::open(drm_plan, kms_device, seat_session.clone())?;
    println!("native DRM backend active: {}", kms.kind().as_str());
    let drm_file_generation = allocate_native_drm_file_generation();
    let kms_policy = KmsPolicy::parse(std::env::var("OBLIVION_ONE_KMS_MODE").ok().as_deref())?;
    println!("native KMS policy requested: {}", kms_policy.as_str());
    let native_syncobj_device = match DrmSyncobjDevice::from_active_drm_file(kms.file()) {
        Ok(device) => Some(device),
        Err(error) if error.kind() == io::ErrorKind::Unsupported => {
            eprintln!("native explicit sync unavailable on active DRM device: {error}");
            None
        }
        Err(error) => return Err(error.into()),
    };
    server.set_native_syncobj_device(native_syncobj_device);
    let drm_timestamp_clock = query_drm_timestamp_clock(kms.file().as_fd())?;
    let presentation_clock = match drm_timestamp_clock {
        DrmTimestampClock::Monotonic => PresentationClock::Monotonic,
        DrmTimestampClock::Realtime => PresentationClock::Realtime,
    };
    server.set_presentation_clock(presentation_clock);
    let mode_preference = NativeModePreference::from_env();
    let target = select_kms_target(kms.file(), mode_preference)?;
    let mode_label = format!(
        "{}x{}@{}",
        target.width, target.height, target.mode.vrefresh
    );
    println!(
        "native scanout target: connector {}, crtc {}, {}x{}@{}Hz ({})",
        target.connector_id,
        target.crtc_id,
        target.width,
        target.height,
        target.mode.vrefresh,
        mode_preference.as_str()
    );
    perf.log("native.kms", || {
        vec![
            NativePerfField::u64("connector", u64::from(target.connector_id)),
            NativePerfField::u64("crtc", u64::from(target.crtc_id)),
            NativePerfField::str("mode", mode_label.clone()),
            NativePerfField::str("policy", mode_preference.as_str()),
            NativePerfField::str("presentation_clock", drm_timestamp_clock.as_str()),
        ]
    });
    let refresh_hz = normalize_refresh_hz(target.mode.vrefresh);
    let refresh_interval_ns = 1_000_000_000 / u64::from(refresh_hz);
    println!(
        "native frame scheduler: {} Hz target, {} us absolute interval",
        refresh_hz,
        refresh_interval_ns / 1_000
    );

    server.set_output_size(target.width, target.height);
    server.set_output_refresh_hz(refresh_hz);
    let scanout_preference = NativeScanoutPreference::from_env();
    let scanout_plan = NativeScanoutPlan::choose(NativeScanoutChoice {
        preference: scanout_preference,
        gbm_available: session_probe.plan.dependencies.gbm_available,
        egl_available: session_probe.plan.dependencies.egl_available,
        page_flip_available: true,
    });
    println!(
        "native scanout backend target: {}",
        scanout_plan.primary.as_str()
    );
    let scanout_target = scanout_plan.primary.as_str();
    let mut scanout = NativeScanoutBackend::open(
        scanout_plan.clone(),
        kms.file(),
        target.width,
        target.height,
        drm_file_generation,
    )?;
    perf.log("native.backend", || {
        vec![
            NativePerfField::str("drm", kms.kind().as_str()),
            NativePerfField::str("scanout", scanout.kind().as_str()),
            NativePerfField::str("scanout_target", scanout_target),
            NativePerfField::str("mode", mode_label.clone()),
            NativePerfField::u64("refresh_hz", u64::from(refresh_hz)),
            NativePerfField::u64("refresh_interval_ns", refresh_interval_ns),
        ]
    });
    let mut frame_renderer = NativeFrameRenderer::default();
    let mut input_state = NativeInputState::new(target.width, target.height);
    let cursor_preference = NativeCursorPreference::from_env();
    println!(
        "native cursor backend target: {}",
        cursor_preference.as_str()
    );
    let pre_kms_hardware_cursor = match cursor_preference {
        NativeCursorPreference::Software => None,
        NativeCursorPreference::Auto | NativeCursorPreference::Hardware => {
            match NativeHardwareCursor::create(kms.file(), target.crtc_id) {
                Ok(cursor) => Some(cursor),
                Err(error) if cursor_preference == NativeCursorPreference::Hardware => {
                    return Err(error.into());
                }
                Err(error) => {
                    eprintln!(
                        "native cursor: hardware cursor unavailable: {error}; using software"
                    );
                    perf.log("native.cursor", || {
                        vec![
                            NativePerfField::str("backend", "software"),
                            NativePerfField::str("policy", cursor_preference.as_str()),
                            NativePerfField::str("fallback", "create_failed"),
                            NativePerfField::str("error", error.to_string()),
                        ]
                    });
                    None
                }
            }
        }
    };
    let mut cursor_render_mode = if pre_kms_hardware_cursor.is_some() {
        NativeCursorRenderMode::Hardware
    } else {
        NativeCursorRenderMode::Software
    };
    let input_plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
        preference: NativeInputBackendPreference::from_env(),
        libseat_available: seat_session.is_some(),
        libinput_available: session_probe.plan.dependencies.libinput_available,
        raw_evdev_available: session_probe.raw_input_device.is_some(),
    });
    println!(
        "native input backend target: {}",
        input_plan.primary.as_str()
    );
    let initial_damage = NativeOutputDamage::full_output(target.width, target.height);
    let initial_paint = match scanout.paint_server_frame(
        &mut frame_renderer,
        &server,
        &input_state,
        cursor_render_mode,
        &initial_damage,
    ) {
        Ok(paint) => paint,
        Err(error)
            if scanout.kind() == NativeScanoutKind::NativeEglGbm
                && scanout_preference != NativeScanoutPreference::NativeEglGbm
                && app_gpu_preference != CompositorAppGpuPreference::Accelerated =>
        {
            let fallback_plan = scanout_plan.after_failed(scanout.kind());
            if fallback_plan.primary == NativeScanoutKind::Unavailable {
                return Err(error.into());
            }
            eprintln!(
                "native scanout: initial native EGL/GBM paint failed: {error}; trying {} fallback",
                fallback_plan.primary.as_str()
            );
            perf.log("native.backend_fallback", || {
                vec![
                    NativePerfField::str("failed", NativeScanoutKind::NativeEglGbm.as_str()),
                    NativePerfField::str("fallback", fallback_plan.primary.as_str()),
                    NativePerfField::str("error", error.to_string()),
                ]
            });
            drop(scanout);
            scanout = NativeScanoutBackend::open(
                fallback_plan,
                kms.file(),
                target.width,
                target.height,
                drm_file_generation,
            )?;
            scanout.paint_server_frame(
                &mut frame_renderer,
                &server,
                &input_state,
                cursor_render_mode,
                &initial_damage,
            )?
        }
        Err(error) => return Err(error.into()),
    }
    .require_rendered("initial native scanout")?;
    println!("native scanout backend active: {}", scanout.kind().as_str());
    let effective_app_gpu_policy =
        resolve_native_app_gpu_policy(app_gpu_preference, scanout.kind())?;
    if scanout.supports_gpu_buffer_protocols() {
        server.enable_gpu_buffer_protocols();
    }
    apply_native_scanout_feedback(&mut server, &scanout);
    println!("native app GPU preference: {}", app_gpu_preference.as_str());
    println!(
        "native app GPU policy effective: {}",
        effective_app_gpu_policy.as_str()
    );
    println!(
        "native app GPU policy reason: active_scanout={}",
        scanout.kind().metric_name()
    );
    perf.log("native.app_gpu_policy", || {
        vec![
            NativePerfField::str("preference", app_gpu_preference.as_str()),
            NativePerfField::str("effective", effective_app_gpu_policy.as_str()),
            NativePerfField::str("active_scanout", scanout.kind().metric_name()),
            NativePerfField::bool(
                "gpu_buffer_protocols",
                server.gpu_buffer_protocols_enabled(),
            ),
        ]
    });
    perf.log("native.frame", || {
        let mut fields = initial_paint.fields();
        fields.extend(initial_damage.fields());
        fields.extend([
            NativePerfField::str("phase", "initial"),
            NativePerfField::str("mode", mode_label.clone()),
            NativePerfField::str("cursor", cursor_render_mode.as_str()),
            NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
            NativePerfField::u64("render_generation", server.render_generation()),
            NativePerfField::str("render_cause", server.render_generation_cause().as_str()),
        ]);
        fields
    });
    let connector_id = ConnectorId::new(target.connector_id)
        .ok_or_else(|| io::Error::other("selected connector ID is zero"))?;
    let crtc_id =
        CrtcId::new(target.crtc_id).ok_or_else(|| io::Error::other("selected CRTC ID is zero"))?;
    let initial_framebuffer = FramebufferId::new(scanout.fb_id())
        .ok_or_else(|| io::Error::other("initial scanout framebuffer ID is zero"))?;
    let mut kms_backend = KmsBackendSelection::initialize(
        kms.file().as_fd(),
        kms_policy,
        connector_id,
        crtc_id,
        target.mode,
        target.width,
        target.height,
        scanout.scanout_format(),
        initial_framebuffer,
    )?;
    println!(
        "native KMS backend active: {}",
        kms_backend.effective_kind().as_str()
    );
    if let Some(reason) = &kms_backend.fallback_reason {
        eprintln!("native KMS: atomic startup unavailable, using legacy: {reason}");
    }
    perf.log("native.kms_backend", || {
        let mut fields = vec![
            NativePerfField::str("requested", kms_policy.as_str()),
            NativePerfField::str("effective", kms_backend.effective_kind().as_str()),
            NativePerfField::u64("connector", u64::from(target.connector_id)),
            NativePerfField::u64("crtc", u64::from(target.crtc_id)),
            NativePerfField::str("mode", mode_label.clone()),
            NativePerfField::str(
                "scanout_format",
                native_visual_label(scanout.scanout_format()),
            ),
        ];
        if let Some(reason) = &kms_backend.fallback_reason {
            fields.push(NativePerfField::str("fallback_reason", reason.to_string()));
        }
        if let Some(atomic) = kms_backend.atomic() {
            fields.extend([
                NativePerfField::u64(
                    "primary_plane",
                    u64::from(atomic.discovery().pipeline.plane.get()),
                ),
                NativePerfField::u64("mode_blob", u64::from(atomic.mode_blob_id().get())),
                NativePerfField::usize("initial_property_count", atomic.initial_property_count()),
                NativePerfField::u64("test_only_us", atomic.test_only_us()),
                NativePerfField::u64("initial_commit_us", atomic.initial_commit_us()),
                NativePerfField::u64(
                    "plane_possible_crtcs",
                    u64::from(atomic.discovery().plane_possible_crtcs),
                ),
                NativePerfField::usize(
                    "plane_format_count",
                    atomic.discovery().plane_formats.len(),
                ),
                NativePerfField::bool("vrr_property", atomic.discovery().optional.vrr_enabled),
                NativePerfField::bool("in_fence_fd", atomic.discovery().optional.in_fence_fd),
                NativePerfField::bool("out_fence_ptr", atomic.discovery().optional.out_fence_ptr),
                NativePerfField::bool(
                    "fb_damage_clips",
                    atomic.discovery().optional.framebuffer_damage_clips,
                ),
            ]);
        }
        fields
    });
    // Keep cursor teardown ahead of KMS restoration on every return path. The
    // cursor is created before the initial paint, but ownership moves here so
    // reverse declaration-order drop disables it before `kms_backend` restores.
    let mut hardware_cursor = pre_kms_hardware_cursor;
    scanout.finish_initial_scanout();
    if let Some(cursor) = hardware_cursor.as_mut() {
        let (cursor_x, cursor_y) = input_state.cursor_position();
        if let Err(error) = cursor
            .enable()
            .and_then(|()| cursor.move_to(cursor_x, cursor_y))
        {
            if cursor_preference == NativeCursorPreference::Hardware {
                return Err(error.into());
            }
            eprintln!("native cursor: hardware cursor activation failed: {error}; using software");
            hardware_cursor = None;
            cursor_render_mode = NativeCursorRenderMode::Software;
            let fallback_damage = NativeOutputDamage::full_output(target.width, target.height);
            let fallback_paint = scanout
                .paint_server_frame(
                    &mut frame_renderer,
                    &server,
                    &input_state,
                    cursor_render_mode,
                    &fallback_damage,
                )?
                .require_rendered("software cursor fallback")?;
            perf.log("native.frame", || {
                let mut fields = fallback_paint.fields();
                fields.extend(fallback_damage.fields());
                fields.extend([
                    NativePerfField::str("phase", "cursor-fallback"),
                    NativePerfField::str("mode", mode_label.clone()),
                    NativePerfField::str("cursor", cursor_render_mode.as_str()),
                    NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                    NativePerfField::u64("render_generation", server.render_generation()),
                    NativePerfField::str("render_cause", server.render_generation_cause().as_str()),
                ]);
                fields
            });
            scanout.present(&kms_backend)?;
            perf.log("native.cursor", || {
                vec![
                    NativePerfField::str("backend", cursor_render_mode.as_str()),
                    NativePerfField::str("policy", cursor_preference.as_str()),
                    NativePerfField::str("fallback", "activation_failed"),
                    NativePerfField::str("error", error.to_string()),
                ]
            });
        } else {
            println!(
                "native cursor backend active: hardware ({}x{})",
                cursor.width, cursor.height
            );
            perf.log("native.cursor", || {
                vec![
                    NativePerfField::str("backend", cursor_render_mode.as_str()),
                    NativePerfField::str("policy", cursor_preference.as_str()),
                    NativePerfField::u64("width", u64::from(cursor.width)),
                    NativePerfField::u64("height", u64::from(cursor.height)),
                ]
            });
        }
    } else {
        println!("native cursor backend active: software");
        perf.log("native.cursor", || {
            vec![
                NativePerfField::str("backend", cursor_render_mode.as_str()),
                NativePerfField::str("policy", cursor_preference.as_str()),
            ]
        });
    }

    println!("native scanout active; press Alt+P to stop");
    let mut input_devices = NativeInputBackend::open(
        input_plan,
        target.width,
        target.height,
        seat_session.clone(),
    )?;
    println!(
        "native input backend active: {}",
        input_devices.kind().as_str()
    );
    perf.log("native.input", || {
        vec![
            NativePerfField::str("backend", input_devices.kind().as_str()),
            NativePerfField::u64("width", u64::from(target.width)),
            NativePerfField::u64("height", u64::from(target.height)),
        ]
    });
    set_fd_nonblocking(kms.file().as_raw_fd())?;
    let acquire_notifier = DrmAcquirePointNotifier;
    let mut acquire_watches =
        ExplicitSyncWatchRegistry::new(refresh_interval_ns, drm_file_generation);
    server.enable_external_acquire_readiness();
    let mut event_loop = NativeEventLoop::new()?;
    event_loop.register(kms.file().as_raw_fd(), NativeEventSource::Drm)?;
    event_loop.register(
        server.listener_fd().as_raw_fd(),
        NativeEventSource::WaylandListener,
    )?;
    event_loop.register(
        server.client_dispatch_fd().as_raw_fd(),
        NativeEventSource::WaylandClients,
    )?;
    for (index, fd) in input_devices.event_fds().enumerate() {
        let index = u16::try_from(index)
            .map_err(|_| io::Error::other("too many native input event sources"))?;
        event_loop.register(fd, NativeEventSource::Input(index))?;
    }
    let scheduler_anchor_ns = monotonic_now_ns()?;
    let mut frame_scheduler = NativeFrameScheduler::new(refresh_hz, scheduler_anchor_ns);
    if let Some(token) = scanout.pending_page_flip_token() {
        frame_scheduler
            .note_async_submission(token, scheduler_anchor_ns)
            .map_err(io::Error::other)?;
    }
    event_loop.arm_deadline(earliest_native_deadline(
        frame_scheduler.next_deadline_ns(),
        acquire_watches.next_fallback_deadline_ns(),
    ))?;
    let mut last_render_generation = server.render_generation();
    let mut last_renderable_surfaces = server.renderable_surfaces().to_vec();
    let mut queued_redraw_requested = false;
    let mut frame_index = 0u64;
    let mut known_toplevels = server.xdg_toplevels();
    let mut pending_launches = VecDeque::<NativeAppLaunchPerf>::new();
    let mut mismatched_pageflip_events = 0u64;
    let mut stale_pageflip_events = 0u64;
    let mut last_acquire_ready_at_ns = None;
    let mut resize_perf = NativeResizePerfState::default();
    let mut pointer_constraint_backend = NativePointerConstraintBackend::new();
    if let Some(command) = startup_app
        && let Some(launch) = launch_native_shell_command(
            &server,
            command,
            effective_app_gpu_policy,
            NativeLaunchSource::Startup,
        )?
    {
        log_native_app_spawn(perf, &launch);
        pending_launches.push_back(launch);
    }
    loop {
        let wakeup = event_loop.wait()?;
        let scheduler_state_before = frame_scheduler.state();
        perf.log("native.wakeup", || {
            vec![
                NativePerfField::u64("ready_mask", u64::from(wakeup.reasons.bits())),
                NativePerfField::usize("ready_sources", wakeup.ready_sources),
                NativePerfField::u64("blocked_us", wakeup.blocked_ns / 1_000),
                NativePerfField::u64(
                    "deadline_late_us",
                    wakeup.timer_lateness_ns.unwrap_or(0) / 1_000,
                ),
                NativePerfField::str("scheduler_before", format!("{scheduler_state_before:?}")),
                NativePerfField::bool("pageflip_pending", scanout.page_flip_pending()),
            ]
        });
        if wakeup.reasons.timer() {
            perf.log("native.deadline", || {
                vec![
                    NativePerfField::u64(
                        "lateness_us",
                        wakeup.timer_lateness_ns.unwrap_or(0) / 1_000,
                    ),
                    NativePerfField::str("scheduler_state", format!("{scheduler_state_before:?}")),
                    NativePerfField::bool("pageflip_watchdog", frame_scheduler.page_flip_pending()),
                ]
            });
        }

        input_devices.dispatch_session_events();
        let pageflip_drain_start = Instant::now();
        let should_drain_pageflips =
            wakeup.reasons.drm() || (wakeup.reasons.timer() && frame_scheduler.page_flip_pending());
        let pageflip_drain = if should_drain_pageflips {
            scanout
                .drain_page_flip_events(kms.file().as_raw_fd())
                .map_err(|error| {
                    native_runtime_error(
                        NativeRuntimeStage::DrainPageFlipEvents,
                        scanout.kind(),
                        target.crtc_id,
                        frame_index,
                        error,
                    )
                })?
        } else {
            NativePageFlipDrain::default()
        };
        let pageflip_drain_us = elapsed_micros(pageflip_drain_start);
        mismatched_pageflip_events =
            mismatched_pageflip_events.saturating_add(pageflip_drain.mismatched_events);
        stale_pageflip_events = stale_pageflip_events.saturating_add(pageflip_drain.stale_events);
        if pageflip_drain.mismatched_events > 0 || pageflip_drain.stale_events > 0 {
            perf.log("native.pageflip_event_error", || {
                vec![
                    NativePerfField::u64("mismatched", pageflip_drain.mismatched_events),
                    NativePerfField::u64("stale", pageflip_drain.stale_events),
                    NativePerfField::u64(
                        "expected_token",
                        pageflip_drain.last_mismatch.map_or(0, |value| value.0),
                    ),
                    NativePerfField::u64(
                        "received_token",
                        pageflip_drain.last_mismatch.map_or(0, |value| value.1),
                    ),
                    NativePerfField::u64(
                        "stale_token",
                        pageflip_drain.last_stale_token.unwrap_or(0),
                    ),
                    NativePerfField::str("kms_backend", kms_backend.effective_kind().as_str()),
                    NativePerfField::u64("backend_generation", drm_file_generation),
                ]
            });
        }
        let pageflip_completed = pageflip_drain.completion.is_some();
        let mut frame_completed = false;
        let mut frame_rendered = false;
        let mut frame_submitted = false;
        if let Some(pageflip) = pageflip_drain.completion {
            let compositor_receive_ns = monotonic_now_ns()?;
            let scheduler_state_at_completion = frame_scheduler.state();
            let completion = frame_scheduler
                .note_page_flip_completion(pageflip.user_data, compositor_receive_ns);
            if let PageFlipCompletionResult::Completed { submitted_at_ns } = completion {
                let presentation = FramePresentation::synchronized(
                    presentation_clock,
                    pageflip.timestamp.seconds,
                    pageflip.timestamp.microseconds,
                    pageflip.sequence,
                )?;
                let compositor_receive_us = sample_clock_microseconds(drm_timestamp_clock)?;
                let kernel_timestamp_us = u64::from(pageflip.timestamp.seconds)
                    .saturating_mul(1_000_000)
                    .saturating_add(u64::from(pageflip.timestamp.microseconds));
                let finish_frame_start = Instant::now();
                server.finish_frame_with_presentation(presentation);
                if !server.has_pending_frame_work() {
                    frame_scheduler.complete_protocol_only();
                }
                frame_completed = true;
                perf.log("native.finish_frame", || {
                    vec![
                        NativePerfField::str("reason", "pageflip_complete"),
                        NativePerfField::u64("elapsed_us", elapsed_micros(finish_frame_start)),
                        NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                        NativePerfField::u64("render_generation", server.render_generation()),
                        NativePerfField::u64("pageflip_token", pageflip.user_data),
                        NativePerfField::str("kms_backend", kms_backend.effective_kind().as_str()),
                        NativePerfField::u64("backend_generation", drm_file_generation),
                        NativePerfField::u64("kernel_sequence", u64::from(pageflip.sequence)),
                        NativePerfField::u64("kernel_timestamp_us", kernel_timestamp_us),
                        NativePerfField::u64("compositor_receive_us", compositor_receive_us),
                        NativePerfField::u64(
                            "receive_delay_us",
                            compositor_receive_us.saturating_sub(kernel_timestamp_us),
                        ),
                        NativePerfField::u64(
                            "submit_to_completion_us",
                            compositor_receive_ns.saturating_sub(submitted_at_ns) / 1_000,
                        ),
                        NativePerfField::str(
                            "scheduler_state",
                            format!("{scheduler_state_at_completion:?}"),
                        ),
                    ]
                });
            }
        }
        let present_us = 0;
        let pageflip_pending_at_tick = scanout.page_flip_pending();
        let tick_start = Instant::now();
        let accepted = server.tick()?;
        let tick_us = elapsed_micros(tick_start);
        let mut redraw_requested = process_native_pointer_constraint_backend_requests(
            &mut server,
            &mut pointer_constraint_backend,
            &mut input_state,
            &mut hardware_cursor,
            cursor_render_mode,
        )?;
        let current_toplevels = server.xdg_toplevels();
        if current_toplevels > known_toplevels {
            for _ in known_toplevels..current_toplevels {
                let app_id = server.last_app_id().unwrap_or("unknown").to_string();
                if let Some(launch) = pending_launches.pop_front() {
                    perf.log("app.first_toplevel", || {
                        vec![
                            NativePerfField::str("program", launch.program.clone()),
                            NativePerfField::str("command", launch.command.clone()),
                            NativePerfField::str("source", launch.source.as_str()),
                            NativePerfField::u64("pid", u64::from(launch.pid)),
                            NativePerfField::str("app_id", app_id.clone()),
                            NativePerfField::u64("spawn_us", launch.spawn_us),
                            NativePerfField::u64("elapsed_us", elapsed_micros(launch.started_at)),
                            NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                        ]
                    });
                } else {
                    perf.log("app.toplevel", || {
                        vec![
                            NativePerfField::str("app_id", app_id.clone()),
                            NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                            NativePerfField::usize("total_toplevels", current_toplevels),
                        ]
                    });
                }
            }
            known_toplevels = current_toplevels;
        }
        if accepted > 0 {
            println!(
                "accepted {accepted} client(s); total {}",
                server.accepted_clients()
            );
        }
        let mut skipped_input_repaints = 0usize;
        let input_drain_start = Instant::now();
        let raw_events = input_devices.drain_events();
        let input_drain_us = elapsed_micros(input_drain_start);
        let raw_input_events = raw_events.len();
        let input_event_timestamp_usec = matches!(
            input_devices.kind(),
            NativeInputBackendKind::LibseatLibinputUdev
                | NativeInputBackendKind::DirectLibinputUdev
        )
        .then(|| {
            raw_events
                .iter()
                .filter_map(|event| event.timestamp_usec())
                .max()
        })
        .flatten();
        let coalesced_events = coalesce_pointer_motion_events(raw_events);
        let coalesced_input_events = coalesced_events.len();
        for event in coalesced_events {
            let may_change_pointer_constraints = event.may_change_pointer_constraints();
            let effect = input_state.handle_hardware_input_event(event);
            let effect_requested_redraw = effect.redraw_requested;
            if let Some((cursor_x, cursor_y)) = effect.cursor_position
                && let Some(cursor) = hardware_cursor.as_mut()
                && let Err(error) = cursor.move_to(cursor_x, cursor_y)
            {
                if cursor_preference == NativeCursorPreference::Hardware {
                    acquire_watches.shutdown(&mut event_loop)?;
                    return Err(error.into());
                }
                eprintln!("native cursor: hardware cursor move failed: {error}; using software");
                hardware_cursor = None;
                cursor_render_mode = NativeCursorRenderMode::Software;
                perf.log("native.cursor", || {
                    vec![
                        NativePerfField::str("backend", cursor_render_mode.as_str()),
                        NativePerfField::str("policy", cursor_preference.as_str()),
                        NativePerfField::str("fallback", "move_failed"),
                        NativePerfField::str("error", error.to_string()),
                    ]
                });
            }
            let application = apply_native_input_effect(
                effect,
                &mut server,
                perf,
                &mut resize_perf,
                cursor_render_mode,
                effective_app_gpu_policy,
            )?;
            if application.exit_requested {
                println!("native input exit requested; shutting down cleanly");
                acquire_watches.shutdown(&mut event_loop)?;
                if let Some(cursor) = hardware_cursor.as_mut() {
                    let _ = cursor.disable();
                }
                let restoration = kms_backend.restore()?;
                perf.log("native.kms_restore", || {
                    vec![
                        NativePerfField::str("backend", kms_backend.effective_kind().as_str()),
                        NativePerfField::str("outcome", restoration.as_str()),
                        NativePerfField::bool("pageflip_pending", scanout.page_flip_pending()),
                    ]
                });
                return Ok(());
            }
            if let Some(launch) = application.launch {
                log_native_app_spawn(perf, &launch);
                pending_launches.push_back(launch);
            }
            if effect_requested_redraw && !application.redraw_requested {
                skipped_input_repaints = skipped_input_repaints.saturating_add(1);
            }
            redraw_requested |= application.redraw_requested;
            if may_change_pointer_constraints {
                let _ = server.tick()?;
                redraw_requested |= process_native_pointer_constraint_backend_requests(
                    &mut server,
                    &mut pointer_constraint_backend,
                    &mut input_state,
                    &mut hardware_cursor,
                    cursor_render_mode,
                )?;
            }
        }
        redraw_requested |= process_native_pointer_constraint_backend_requests(
            &mut server,
            &mut pointer_constraint_backend,
            &mut input_state,
            &mut hardware_cursor,
            cursor_render_mode,
        )?;
        if let Some(event_timestamp_us) = input_event_timestamp_usec {
            let dispatch_latency_us = monotonic_now_ns()?
                .saturating_div(1_000)
                .saturating_sub(event_timestamp_us);
            perf.log("native.input_dispatch", || {
                vec![
                    NativePerfField::usize("events", coalesced_input_events),
                    NativePerfField::u64("event_timestamp_us", event_timestamp_us),
                    NativePerfField::u64("dispatch_latency_us", dispatch_latency_us),
                ]
            });
        }
        let acquire_changes = server.take_acquire_watch_changes();
        let acquire_change_count = acquire_changes.len();
        let acquire_ready_token_count = wakeup.explicit_sync_acquire_tokens.len();
        let mut acquire_ready_count = 0usize;
        for change in acquire_changes {
            match change {
                AcquireWatchChange::Register(request) => {
                    match acquire_watches.register(
                        request,
                        &mut event_loop,
                        monotonic_now_ns()?,
                        &acquire_notifier,
                    )? {
                        AcquireRegistrationResult::AlreadyReady(request) => {
                            if server.mark_acquire_commit_ready(
                                request.commit_id,
                                request.surface_id,
                                &request.acquire,
                            ) {
                                acquire_ready_count = acquire_ready_count.saturating_add(1);
                            }
                        }
                        AcquireRegistrationResult::EventfdBacked(commit_id) => {
                            let _ = server.mark_acquire_commit_eventfd_backed(commit_id);
                        }
                        AcquireRegistrationResult::FallbackBacked(commit_id) => {
                            let _ = server.mark_acquire_commit_fallback_backed(commit_id);
                        }
                    }
                }
                AcquireWatchChange::Cancel { commit_id, reason } => {
                    let _ = acquire_watches.cancel_commit(commit_id, reason, &mut event_loop)?;
                }
            }
        }
        for token in wakeup.explicit_sync_acquire_tokens.iter().copied() {
            match acquire_watches.handle_ready(
                token,
                &mut event_loop,
                drm_file_generation,
                &acquire_notifier,
            )? {
                AcquireReadyResult::Ready(request) => {
                    if server.mark_acquire_commit_ready(
                        request.commit_id,
                        request.surface_id,
                        &request.acquire,
                    ) {
                        acquire_ready_count = acquire_ready_count.saturating_add(1);
                    }
                }
                AcquireReadyResult::BackendMismatch(_) => {}
                AcquireReadyResult::Pending | AcquireReadyResult::Stale => {}
            }
        }
        for request in acquire_watches.retry_fallback(monotonic_now_ns()?, &acquire_notifier) {
            if server.mark_acquire_commit_ready(
                request.commit_id,
                request.surface_id,
                &request.acquire,
            ) {
                acquire_ready_count = acquire_ready_count.saturating_add(1);
            }
        }
        if acquire_change_count > 0 || acquire_ready_token_count > 0 || acquire_ready_count > 0 {
            if acquire_ready_count > 0 {
                last_acquire_ready_at_ns = Some(monotonic_now_ns()?);
            }
            let metrics = acquire_watches.metrics();
            perf.log("native.explicit_sync", || {
                vec![
                    NativePerfField::usize("changes", acquire_change_count),
                    NativePerfField::usize("ready_tokens", acquire_ready_token_count),
                    NativePerfField::usize("ready_commits", acquire_ready_count),
                    NativePerfField::usize(
                        "active_eventfd_watches",
                        metrics.active_eventfd_watches,
                    ),
                    NativePerfField::usize(
                        "active_fallback_watches",
                        metrics.active_fallback_watches,
                    ),
                    NativePerfField::u64("registrations", metrics.registrations),
                    NativePerfField::u64("already_signaled", metrics.already_signaled),
                    NativePerfField::u64("eventfd_wakeups", metrics.eventfd_wakeups),
                    NativePerfField::u64("stale_wakeups", metrics.stale_wakeups),
                    NativePerfField::u64("duplicate_wakeups", metrics.duplicate_wakeups),
                    NativePerfField::u64("cancellations", metrics.cancellations),
                    NativePerfField::u64("registration_failures", metrics.registration_failures),
                    NativePerfField::u64(
                        "last_registration_errno",
                        metrics.last_registration_errno.max(0) as u64,
                    ),
                    NativePerfField::u64(
                        "commit_to_acquire_ready_us",
                        metrics.last_commit_to_ready_ns / 1_000,
                    ),
                    NativePerfField::u64("fallback_activations", metrics.fallback_activations),
                    NativePerfField::usize(
                        "maximum_simultaneous_watches",
                        metrics.maximum_simultaneous_watches,
                    ),
                    NativePerfField::u64(
                        "leaked_watch_assertions",
                        metrics.leaked_watch_assertions,
                    ),
                    NativePerfField::u64("canceled_superseded", metrics.cancellations_by_reason[0]),
                    NativePerfField::u64(
                        "canceled_surface_destroyed",
                        metrics.cancellations_by_reason[1],
                    ),
                    NativePerfField::u64(
                        "canceled_buffer_destroyed",
                        metrics.cancellations_by_reason[2],
                    ),
                    NativePerfField::u64(
                        "canceled_sync_surface_destroyed",
                        metrics.cancellations_by_reason[3],
                    ),
                    NativePerfField::u64(
                        "canceled_timeline_destroyed",
                        metrics.cancellations_by_reason[4],
                    ),
                    NativePerfField::u64(
                        "canceled_client_disconnected",
                        metrics.cancellations_by_reason[5],
                    ),
                ]
            });
        }
        if !scanout.page_flip_pending() && server.has_pending_frame_prepare_work() {
            let prepare_frame_start = Instant::now();
            let before_generation = server.render_generation();
            server.prepare_frame();
            let after_generation = server.render_generation();
            let resize = server.resize_flow_metrics();
            let subsurface = server.subsurface_transaction_metrics();
            perf.log("native.prepare_frame", || {
                vec![
                    NativePerfField::u64("elapsed_us", elapsed_micros(prepare_frame_start)),
                    NativePerfField::u64("render_generation", after_generation),
                    NativePerfField::bool("render_changed", after_generation != before_generation),
                    NativePerfField::bool("pending_frame_work", server.has_pending_frame_work()),
                    NativePerfField::u64(
                        "resize_configures_requested",
                        resize.configures_requested,
                    ),
                    NativePerfField::u64("resize_configures_sent", resize.configures_sent),
                    NativePerfField::u64(
                        "resize_geometries_coalesced",
                        resize.geometries_coalesced,
                    ),
                    NativePerfField::u64("resize_acks_matched", resize.acks_matched),
                    NativePerfField::u64("resize_acks_stale", resize.acks_stale),
                    NativePerfField::u64("resize_acks_unknown", resize.acks_unknown),
                    NativePerfField::u64("resize_commits_captured", resize.commits_captured),
                    NativePerfField::u64(
                        "resize_interactions_started",
                        resize.resize_interactions_started,
                    ),
                    NativePerfField::u64(
                        "resize_rapid_reresize_interactions",
                        resize.rapid_reresize_interactions,
                    ),
                    NativePerfField::u64(
                        "resize_obsolete_finals_discarded",
                        resize.obsolete_finals_discarded,
                    ),
                    NativePerfField::u64(
                        "resize_obsolete_queued_targets_discarded",
                        resize.obsolete_queued_targets_discarded,
                    ),
                    NativePerfField::u64(
                        "resize_stale_interaction_commits_applied",
                        resize.stale_interaction_commits_applied,
                    ),
                    NativePerfField::u64(
                        "resize_stale_commits_preserved_preview",
                        resize.stale_commits_preserved_preview,
                    ),
                    NativePerfField::u64(
                        "resize_preview_ownership_transfers",
                        resize.preview_ownership_transfers,
                    ),
                    NativePerfField::u64(
                        "resize_final_configures_sent",
                        resize.final_configures_sent,
                    ),
                    NativePerfField::u64(
                        "resize_interactions_completed",
                        resize.resize_interactions_completed,
                    ),
                    NativePerfField::u64(
                        "resize_interactions_canceled",
                        resize.resize_interactions_canceled,
                    ),
                    NativePerfField::u64(
                        "resize_visual_geometry_starts",
                        resize.visual_geometry_resize_starts,
                    ),
                    NativePerfField::u64(
                        "resize_raw_pointer_updates",
                        resize.raw_pointer_resize_updates,
                    ),
                    NativePerfField::u64(
                        "resize_pending_updates_replaced",
                        resize.pending_resize_updates_replaced,
                    ),
                    NativePerfField::u64("resize_updates_applied", resize.resize_updates_applied),
                    NativePerfField::u64(
                        "resize_updates_skipped_unchanged",
                        resize.resize_updates_skipped_unchanged,
                    ),
                    NativePerfField::u64(
                        "resize_duplicate_configures_skipped",
                        resize.duplicate_configure_sizes_skipped,
                    ),
                    NativePerfField::usize(
                        "resize_max_retained_configures",
                        resize.maximum_retained_configures,
                    ),
                    NativePerfField::u64("resize_preview_max_age_ms", resize.max_preview_age_ms),
                    NativePerfField::usize("resize_max_in_flight", resize.max_in_flight_configures),
                    NativePerfField::usize(
                        "resize_max_pending_explicit_sync",
                        resize.max_pending_explicit_sync_commits,
                    ),
                    NativePerfField::u64(
                        "subsurface_commits_cached",
                        subsurface.synchronized_child_commits_cached,
                    ),
                    NativePerfField::u64(
                        "subsurface_commits_merged",
                        subsurface.cached_commits_merged,
                    ),
                    NativePerfField::u64(
                        "subsurface_trees_published",
                        subsurface.tree_transactions_published,
                    ),
                    NativePerfField::u64(
                        "subsurface_trees_waiting_acquire",
                        subsurface.tree_transactions_waiting_on_acquire,
                    ),
                    NativePerfField::u64(
                        "subsurface_bufferless_tree_commits_merged",
                        subsurface.bufferless_tree_commits_merged,
                    ),
                    NativePerfField::u64(
                        "subsurface_metadata_only_nodes_merged",
                        subsurface.metadata_only_nodes_merged,
                    ),
                    NativePerfField::u64(
                        "subsurface_attachments_replaced",
                        subsurface.attachments_replaced,
                    ),
                    NativePerfField::u64(
                        "subsurface_explicit_detaches",
                        subsurface.explicit_detaches,
                    ),
                    NativePerfField::u64(
                        "subsurface_acquire_dependencies_preserved",
                        subsurface.acquire_dependencies_preserved,
                    ),
                    NativePerfField::u64(
                        "subsurface_acquire_dependencies_replaced",
                        subsurface.acquire_dependencies_replaced,
                    ),
                    NativePerfField::u64(
                        "subsurface_ready_preserved_from_newer_unready",
                        subsurface.ready_transactions_preserved_from_newer_unready,
                    ),
                    NativePerfField::u64(
                        "subsurface_callbacks_merged",
                        subsurface.callbacks_merged,
                    ),
                    NativePerfField::u64(
                        "subsurface_feedbacks_merged",
                        subsurface.feedbacks_merged,
                    ),
                    NativePerfField::u64(
                        "subsurface_resize_snapshots_preserved",
                        subsurface.resize_snapshots_preserved,
                    ),
                    NativePerfField::u64(
                        "subsurface_resize_snapshots_replaced",
                        subsurface.resize_snapshots_replaced,
                    ),
                    NativePerfField::u64(
                        "subsurface_root_wide_supersessions",
                        subsurface.root_wide_supersessions,
                    ),
                    NativePerfField::u64(
                        "subsurface_waiting_transactions_published",
                        subsurface.waiting_transactions_published,
                    ),
                    NativePerfField::usize(
                        "subsurface_max_ready_slots_per_root",
                        subsurface.maximum_ready_slots_per_root,
                    ),
                    NativePerfField::usize(
                        "subsurface_max_waiting_slots_per_root",
                        subsurface.maximum_waiting_slots_per_root,
                    ),
                    NativePerfField::usize(
                        "subsurface_max_cached_nodes",
                        subsurface.maximum_cached_nodes,
                    ),
                    NativePerfField::usize(
                        "subsurface_max_tree_depth",
                        subsurface.maximum_tree_depth,
                    ),
                    NativePerfField::u64(
                        "subsurface_max_wait_ms",
                        subsurface.maximum_transaction_wait_ms,
                    ),
                ]
            });
        }
        let render_generation = server.render_generation();
        let render_generation_changed = render_generation != last_render_generation;
        let render_generation_cause = server.render_generation_cause();
        let pending_frame_work = server.has_pending_frame_work();
        let repaint_decision = native_repaint_decision(NativeRepaintInputs {
            accepted_clients: accepted > 0,
            render_generation_changed,
            pending_frame_work,
            only_pending_surface_frame_callbacks: server.has_only_pending_surface_frame_callbacks(),
            redraw_requested,
            page_flip_pending: false,
        });
        if repaint_decision.repaint {
            frame_scheduler.queue_visual_work();
            queued_redraw_requested |= redraw_requested;
        } else if repaint_decision.protocol_only_present {
            frame_scheduler.queue_protocol_work(monotonic_now_ns()?);
        }
        let scheduler_decision = frame_scheduler.decision(monotonic_now_ns()?);
        if scheduler_decision == SchedulerDecision::PageFlipWatchdogExpired {
            perf.log("native.pageflip_watchdog", || {
                vec![
                    NativePerfField::u64("frame", frame_index),
                    NativePerfField::u64("crtc", u64::from(target.crtc_id)),
                    NativePerfField::str("scanout", scanout.kind().metric_name()),
                    NativePerfField::str("kms_backend", kms_backend.effective_kind().as_str()),
                    NativePerfField::u64(
                        "pending_token",
                        scanout.pending_page_flip_token().unwrap_or(0),
                    ),
                    NativePerfField::u64("backend_generation", drm_file_generation),
                    NativePerfField::u64("timeout_count", frame_scheduler.watchdog_timeout_count()),
                    NativePerfField::bool("drm_ready", wakeup.reasons.drm()),
                    NativePerfField::bool("final_drain_completed", pageflip_completed),
                ]
            });
            acquire_watches.shutdown(&mut event_loop)?;
            return Err(io::Error::other(format!(
                "native page flip watchdog expired: backend={} crtc={} frame={} pending=true; final DRM drain found no completion",
                scanout.kind().metric_name(),
                target.crtc_id,
                frame_index
            ))
            .into());
        }
        if scheduler_decision == SchedulerDecision::Render {
            let effective_redraw_requested = redraw_requested || queued_redraw_requested;
            let render_cause = native_repaint_cause_label(
                render_generation_cause,
                render_generation_changed,
                accepted,
                pending_frame_work,
                effective_redraw_requested,
            );
            let output_damage = native_output_damage_for_repaint(
                target.width,
                target.height,
                &last_renderable_surfaces,
                server.renderable_surfaces(),
                render_generation_cause,
                render_generation_changed,
            );
            let skip_empty_visible_damage = output_damage.is_empty()
                && render_generation_changed
                && accepted == 0
                && !effective_redraw_requested;
            if skip_empty_visible_damage {
                perf.log("native.frame_skip", || {
                    let mut fields = output_damage.fields().to_vec();
                    fields.extend([
                        NativePerfField::str("reason", "empty_visible_damage"),
                        NativePerfField::usize("skipped_input_repaints", skipped_input_repaints),
                        NativePerfField::u64("tick_us", tick_us),
                        NativePerfField::bool("pageflip_pending_at_tick", pageflip_pending_at_tick),
                        NativePerfField::u64("input_drain_us", input_drain_us),
                        NativePerfField::usize("raw_input_events", raw_input_events),
                        NativePerfField::usize("coalesced_input_events", coalesced_input_events),
                        NativePerfField::u64("pageflip_drain_us", pageflip_drain_us),
                        NativePerfField::bool("pageflip_completed", pageflip_completed),
                        NativePerfField::u64("present_us", present_us),
                        NativePerfField::str("kms_backend", kms_backend.effective_kind().as_str()),
                        NativePerfField::u64(
                            "pageflip_token",
                            scanout.pending_page_flip_token().unwrap_or(0),
                        ),
                        NativePerfField::u64("backend_generation", drm_file_generation),
                        NativePerfField::u64("render_generation", render_generation),
                        NativePerfField::str("render_cause", render_cause),
                        NativePerfField::bool("pending_frame_work", pending_frame_work),
                    ]);
                    fields
                });
                if pending_frame_work {
                    let finish_frame_start = Instant::now();
                    server.finish_frame();
                    perf.log("native.finish_frame", || {
                        vec![
                            NativePerfField::str("reason", "empty_visible_damage"),
                            NativePerfField::u64("elapsed_us", elapsed_micros(finish_frame_start)),
                            NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                            NativePerfField::u64("render_generation", server.render_generation()),
                        ]
                    });
                }
                frame_scheduler.note_immediate_completion();
                queued_redraw_requested = false;
                last_render_generation = render_generation;
                last_renderable_surfaces = server.renderable_surfaces().to_vec();
            } else {
                let cpu_before = perf
                    .enabled()
                    .then(NativeProcessCpuSample::read_current)
                    .flatten();
                let paint_outcome = scanout.paint_server_frame(
                    &mut frame_renderer,
                    &server,
                    &input_state,
                    cursor_render_mode,
                    &output_damage,
                )?;
                let paint_stats = paint_outcome.stats();
                if matches!(paint_outcome, NativePaintOutcome::Skipped(_)) {
                    frame_scheduler.note_immediate_completion();
                    if server.has_pending_frame_work() {
                        server.finish_frame();
                        frame_completed = true;
                    }
                    perf.log("native.frame_skip", || {
                        let mut fields = paint_stats.fields();
                        fields.extend(output_damage.fields());
                        fields.extend([
                            NativePerfField::str("reason", "renderer_no_logical_damage"),
                            NativePerfField::bool("egl_swap_attempted", false),
                            NativePerfField::bool("gbm_front_buffer_locked", false),
                            NativePerfField::bool("ready_frame_created", false),
                            NativePerfField::u64("render_generation", render_generation),
                        ]);
                        fields
                    });
                    queued_redraw_requested = false;
                    last_render_generation = render_generation;
                    last_renderable_surfaces = server.renderable_surfaces().to_vec();
                } else {
                    frame_rendered = true;
                    let cpu_after = perf
                        .enabled()
                        .then(NativeProcessCpuSample::read_current)
                        .flatten();
                    let (cpu_user_us, cpu_system_us) = cpu_before
                        .zip(cpu_after)
                        .map(|(before, after)| after.delta_us_since(before))
                        .unwrap_or((0, 0));
                    let repaint_present_start = Instant::now();
                    let present_result = scanout.present(&kms_backend).map_err(|error| {
                        native_runtime_error(
                            NativeRuntimeStage::Present,
                            scanout.kind(),
                            target.crtc_id,
                            frame_index,
                            error,
                        )
                    })?;
                    let repaint_present_us = elapsed_micros(repaint_present_start);
                    let acquire_ready_to_render_submit_us = last_acquire_ready_at_ns
                        .map(|ready_at| {
                            monotonic_now_ns().map(|now| now.saturating_sub(ready_at) / 1_000)
                        })
                        .transpose()?
                        .unwrap_or(0);
                    match present_result {
                        NativePresentResult::AsyncSubmitted { token } => {
                            frame_scheduler
                                .note_async_submission(token, monotonic_now_ns()?)
                                .map_err(io::Error::other)?;
                            frame_submitted = true;
                        }
                        NativePresentResult::Immediate => {
                            frame_scheduler.note_immediate_completion();
                            if server.has_pending_frame_work() {
                                let finish_frame_start = Instant::now();
                                server.finish_frame();
                                frame_completed = true;
                                perf.log("native.finish_frame", || {
                                    vec![
                                        NativePerfField::str("reason", "immediate_scanout"),
                                        NativePerfField::u64(
                                            "elapsed_us",
                                            elapsed_micros(finish_frame_start),
                                        ),
                                        NativePerfField::usize(
                                            "surfaces",
                                            server.renderable_surfaces().len(),
                                        ),
                                        NativePerfField::u64(
                                            "render_generation",
                                            server.render_generation(),
                                        ),
                                    ]
                                });
                            }
                        }
                        NativePresentResult::Noop => {
                            return Err(io::Error::other(
                                "native scanout rendered a frame but did not submit or complete it",
                            )
                            .into());
                        }
                    }
                    server.mark_render_damage_presented();
                    last_acquire_ready_at_ns = None;
                    frame_index = frame_index.saturating_add(1);
                    perf.log("native.frame", || {
                        let mut fields = paint_stats.fields();
                        fields.extend(output_damage.fields());
                        fields.extend([
                            NativePerfField::u64("index", frame_index),
                            NativePerfField::str("phase", "repaint"),
                            NativePerfField::str("mode", mode_label.clone()),
                            NativePerfField::str("cursor", cursor_render_mode.as_str()),
                            NativePerfField::u64("refresh_hz", u64::from(refresh_hz)),
                            NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                            NativePerfField::u64("render_generation", render_generation),
                            NativePerfField::bool("render_changed", render_generation_changed),
                            NativePerfField::str("render_cause", render_cause),
                            NativePerfField::u64("tick_us", tick_us),
                            NativePerfField::bool(
                                "pageflip_pending_at_tick",
                                pageflip_pending_at_tick,
                            ),
                            NativePerfField::u64("input_drain_us", input_drain_us),
                            NativePerfField::usize("raw_input_events", raw_input_events),
                            NativePerfField::usize(
                                "coalesced_input_events",
                                coalesced_input_events,
                            ),
                            NativePerfField::u64("pageflip_drain_us", pageflip_drain_us),
                            NativePerfField::bool("pageflip_completed", pageflip_completed),
                            NativePerfField::u64("present_us", present_us),
                            NativePerfField::u64("repaint_present_us", repaint_present_us),
                            NativePerfField::u64(
                                "acquire_ready_to_render_submit_us",
                                acquire_ready_to_render_submit_us,
                            ),
                            NativePerfField::u64("cpu_user_us", cpu_user_us),
                            NativePerfField::u64("cpu_system_us", cpu_system_us),
                            NativePerfField::bool("pending_frame_work", pending_frame_work),
                            NativePerfField::bool("redraw_requested", redraw_requested),
                            NativePerfField::usize(
                                "skipped_input_repaints",
                                skipped_input_repaints,
                            ),
                            NativePerfField::usize("accepted_clients", accepted),
                        ]);
                        fields
                    });
                    queued_redraw_requested = false;
                    last_render_generation = render_generation;
                    last_renderable_surfaces = server.renderable_surfaces().to_vec();
                }
            }
        } else if scheduler_decision == SchedulerDecision::CompleteProtocolOnly {
            perf.log("native.frame_skip", || {
                vec![
                    NativePerfField::str("reason", "frame_callback_no_damage"),
                    NativePerfField::usize("skipped_input_repaints", skipped_input_repaints),
                    NativePerfField::u64("tick_us", tick_us),
                    NativePerfField::bool("pageflip_pending_at_tick", pageflip_pending_at_tick),
                    NativePerfField::u64("input_drain_us", input_drain_us),
                    NativePerfField::usize("raw_input_events", raw_input_events),
                    NativePerfField::usize("coalesced_input_events", coalesced_input_events),
                    NativePerfField::u64("pageflip_drain_us", pageflip_drain_us),
                    NativePerfField::bool("pageflip_completed", pageflip_completed),
                    NativePerfField::u64("present_us", present_us),
                    NativePerfField::u64("render_generation", render_generation),
                ]
            });
            let finish_frame_start = Instant::now();
            server.finish_frame();
            frame_scheduler.complete_protocol_only();
            frame_completed = true;
            perf.log("native.finish_frame", || {
                vec![
                    NativePerfField::str("reason", "frame_callback_no_damage"),
                    NativePerfField::u64("elapsed_us", elapsed_micros(finish_frame_start)),
                    NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                    NativePerfField::u64("render_generation", server.render_generation()),
                ]
            });
        } else if scheduler_decision == SchedulerDecision::WaitForPageFlip {
            perf.log("native.frame_skip", || {
                vec![
                    NativePerfField::str("reason", "pageflip_pending"),
                    NativePerfField::usize("skipped_input_repaints", skipped_input_repaints),
                    NativePerfField::u64("tick_us", tick_us),
                    NativePerfField::bool("pageflip_pending_at_tick", pageflip_pending_at_tick),
                    NativePerfField::u64("input_drain_us", input_drain_us),
                    NativePerfField::usize("raw_input_events", raw_input_events),
                    NativePerfField::usize("coalesced_input_events", coalesced_input_events),
                    NativePerfField::u64("pageflip_drain_us", pageflip_drain_us),
                    NativePerfField::bool("pageflip_completed", pageflip_completed),
                    NativePerfField::u64("present_us", present_us),
                    NativePerfField::u64("render_generation", render_generation),
                    NativePerfField::bool("render_changed", render_generation_changed),
                    NativePerfField::bool("pending_frame_work", pending_frame_work),
                    NativePerfField::bool("redraw_requested", redraw_requested),
                ]
            });
        } else if skipped_input_repaints > 0 {
            perf.log("native.frame_skip", || {
                vec![
                    NativePerfField::str("reason", "input_forwarded_no_visual"),
                    NativePerfField::usize("skipped_input_repaints", skipped_input_repaints),
                    NativePerfField::u64("tick_us", tick_us),
                    NativePerfField::bool("pageflip_pending_at_tick", pageflip_pending_at_tick),
                    NativePerfField::u64("input_drain_us", input_drain_us),
                    NativePerfField::usize("raw_input_events", raw_input_events),
                    NativePerfField::usize("coalesced_input_events", coalesced_input_events),
                    NativePerfField::u64("pageflip_drain_us", pageflip_drain_us),
                    NativePerfField::bool("pageflip_completed", pageflip_completed),
                    NativePerfField::u64("present_us", present_us),
                    NativePerfField::u64("render_generation", render_generation),
                ]
            });
        }
        perf.log("native.scheduler", || {
            vec![
                NativePerfField::str("decision", format!("{scheduler_decision:?}")),
                NativePerfField::str("state_after", format!("{:?}", frame_scheduler.state())),
                NativePerfField::bool("pageflip_pending", frame_scheduler.page_flip_pending()),
                NativePerfField::bool("visual_work_queued", frame_scheduler.visual_work_queued()),
                NativePerfField::bool(
                    "protocol_work_queued",
                    frame_scheduler.protocol_work_queued(),
                ),
                NativePerfField::bool("frame_rendered", frame_rendered),
                NativePerfField::bool("frame_submitted", frame_submitted),
                NativePerfField::bool("frame_completed", frame_completed),
                NativePerfField::u64(
                    "watchdog_timeout_count",
                    frame_scheduler.watchdog_timeout_count(),
                ),
                NativePerfField::u64("mismatched_pageflip_events", mismatched_pageflip_events),
                NativePerfField::u64("stale_pageflip_events", stale_pageflip_events),
            ]
        });
        event_loop.arm_deadline(earliest_native_deadline(
            frame_scheduler.next_deadline_ns(),
            acquire_watches.next_fallback_deadline_ns(),
        ))?;
    }
}
