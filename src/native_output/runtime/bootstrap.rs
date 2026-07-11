use super::*;

pub(super) fn log_native_runtime_bootstrap(
    server: &OwnCompositorServer,
    bootstrap: &NativeOutputBootstrap,
    session_probe: &NativeSessionProbe,
    vrr_plan: NativeVrrPlan,
    startup_app: Option<&Vec<String>>,
    perf: NativePerfLogger,
) {
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
    if let Some(command) = startup_app {
        println!("startup app command deferred until native scanout is ready: {command:?}");
        perf.log("app.deferred", || {
            vec![
                NativePerfField::usize("argc", command.len()),
                NativePerfField::str("command", command.join(" ")),
            ]
        });
    }
}

struct NativeRuntimeBootstrapTail {
    server: OwnCompositorServer,
    perf: NativePerfLogger,
    kms: NativeDrmDevice,
    kms_backend: KmsBackendSelection,
    target: KmsTarget,
    mode_label: String,
    refresh_hz: u32,
    refresh_interval_ns: u64,
    drm_file_generation: u64,
    drm_timestamp_clock: DrmTimestampClock,
    presentation_clock: PresentationClock,
    scanout: NativeScanoutBackend,
    frame_renderer: NativeFrameRenderer,
    input_state: NativeInputState,
    cursor_preference: NativeCursorPreference,
    pre_kms_hardware_cursor: Option<NativeHardwareCursor>,
    cursor_render_mode: NativeCursorRenderMode,
    input_plan: NativeInputBackendPlan,
    seat_session: Option<NativeSeatSession>,
    startup_app: Option<Vec<String>>,
    effective_app_gpu_policy: EffectiveCompositorAppGpuPolicy,
}

impl NativeRuntime {
    fn finish_bootstrap(parts: NativeRuntimeBootstrapTail) -> NativeResult<Self> {
        let NativeRuntimeBootstrapTail {
            mut server,
            perf,
            kms,
            kms_backend,
            target,
            mode_label,
            refresh_hz,
            refresh_interval_ns,
            drm_file_generation,
            drm_timestamp_clock,
            presentation_clock,
            mut scanout,
            mut frame_renderer,
            input_state,
            cursor_preference,
            pre_kms_hardware_cursor,
            mut cursor_render_mode,
            input_plan,
            seat_session,
            startup_app,
            effective_app_gpu_policy,
        } = parts;
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
                eprintln!(
                    "native cursor: hardware cursor activation failed: {error}; using software"
                );
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
                        NativePerfField::str(
                            "render_cause",
                            server.render_generation_cause().as_str(),
                        ),
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
        let input_devices = NativeInputBackend::open(
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
        let acquire_watches =
            ExplicitSyncWatchRegistry::new(refresh_interval_ns, drm_file_generation);
        server.enable_external_acquire_readiness();
        let mut event_loop = NativeEventLoop::new()?;
        let mut process_supervisor = ChildSupervisor::with_sigchld_reaper()?;
        let drm_reactor_token =
            event_loop.register(kms.file().as_raw_fd(), NativeEventSource::Drm)?;
        if let Some(session) = seat_session.as_ref() {
            event_loop.register(session.event_fd()?, NativeEventSource::Seat)?;
        }
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
        if let Some(fd) = process_supervisor.signal_fd() {
            event_loop.register(fd, NativeEventSource::ChildSignal)?;
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
        let last_render_generation = server.render_generation();
        let last_renderable_surfaces = server.renderable_surfaces().to_vec();
        let queued_redraw_requested = false;
        let frame_index = 0u64;
        let known_toplevels = server.xdg_toplevels();
        let mut pending_launches = VecDeque::<NativeAppLaunchPerf>::new();
        let mismatched_pageflip_events = 0u64;
        let stale_pageflip_events = 0u64;
        let presentation_cadence = PresentationCadenceMetrics::default();
        let last_acquire_ready_at_ns = None;
        let resize_perf = NativeResizePerfState::default();
        let pointer_constraint_backend = NativePointerConstraintBackend::new();
        if let Some(command) = external_shell_command()
            && let Some(launch) = launch_native_shell_command(
                &server,
                &mut process_supervisor,
                command,
                effective_app_gpu_policy,
                NativeLaunchSource::ExternalShell,
            )
            .map_err(|error| {
                eprintln!("native external shell launch failed: {error}");
                error
            })
            .ok()
            .flatten()
        {
            server.authorize_astrea_shell_pid(launch.pid);
            log_native_app_spawn(perf, &launch);
            pending_launches.push_back(launch);
        }
        if let Some(command) = startup_app
            && let Some(launch) = launch_native_shell_command(
                &server,
                &mut process_supervisor,
                command,
                effective_app_gpu_policy,
                NativeLaunchSource::Startup,
            )?
        {
            log_native_app_spawn(perf, &launch);
            pending_launches.push_back(launch);
        }
        Ok(Self {
            server,
            perf,
            kms,
            kms_backend,
            target,
            mode_label,
            refresh_hz,
            drm_file_generation,
            drm_timestamp_clock,
            presentation_clock,
            scanout: mem::ManuallyDrop::new(scanout),
            frame_renderer,
            input_state,
            cursor_preference,
            cursor_render_mode,
            hardware_cursor,
            input_devices,
            seat_session,
            session: NativeSessionLifecycle::default(),
            pending_session_recovery: None,
            #[cfg(test)]
            native_io_recorder: NativeIoRecorder::default(),
            acquire_notifier,
            acquire_watches,
            parked_acquire_watches: Vec::new(),
            event_loop,
            drm_reactor_token: Some(drm_reactor_token),
            frame_scheduler,
            effective_app_gpu_policy,
            last_render_generation,
            last_renderable_surfaces,
            queued_redraw_requested,
            frame_index,
            known_toplevels,
            pending_launches,
            mismatched_pageflip_events,
            stale_pageflip_events,
            presentation_cadence,
            last_acquire_ready_at_ns,
            resize_perf,
            pointer_constraint_backend,
            process_supervisor,
            astrea_launch_tracker: AstreaLaunchLifecycleTracker::default(),
            shutdown: NativeShutdownLifecycle::new(),
        })
    }

    pub(super) fn bootstrap_native(config: NativeRuntimeConfig) -> NativeResult<Self> {
        let NativeRuntimeConfig {
            mut server,
            app,
            app_gpu_preference,
        } = config;
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

        log_native_runtime_bootstrap(
            &server,
            &bootstrap,
            &session_probe,
            vrr_plan,
            startup_app.as_ref(),
            perf,
        );

        let Some(kms_device) = bootstrap.kms_device.as_deref() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "native DRM initialization: no /dev/dri/card* KMS device found",
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
        let input_state = NativeInputState::new(target.width, target.height);
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
        let cursor_render_mode = if pre_kms_hardware_cursor.is_some() {
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
        let crtc_id = CrtcId::new(target.crtc_id)
            .ok_or_else(|| io::Error::other("selected CRTC ID is zero"))?;
        let initial_framebuffer = FramebufferId::new(scanout.fb_id())
            .ok_or_else(|| io::Error::other("initial scanout framebuffer ID is zero"))?;
        let kms_backend = KmsBackendSelection::initialize(
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
                    NativePerfField::usize(
                        "initial_property_count",
                        atomic.initial_property_count(),
                    ),
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
                    NativePerfField::bool(
                        "out_fence_ptr",
                        atomic.discovery().optional.out_fence_ptr,
                    ),
                    NativePerfField::bool(
                        "fb_damage_clips",
                        atomic.discovery().optional.framebuffer_damage_clips,
                    ),
                ]);
            }
            fields
        });
        Self::finish_bootstrap(NativeRuntimeBootstrapTail {
            server,
            perf,
            kms,
            kms_backend,
            target,
            mode_label,
            refresh_hz,
            refresh_interval_ns,
            drm_file_generation,
            drm_timestamp_clock,
            presentation_clock,
            scanout,
            frame_renderer,
            input_state,
            cursor_preference,
            pre_kms_hardware_cursor,
            cursor_render_mode,
            input_plan,
            seat_session,
            startup_app,
            effective_app_gpu_policy,
        })
    }
}
