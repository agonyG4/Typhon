use super::*;
use oblivion_one::cursor_theme::{
    CompositorCursorImage, install_shared_compositor_cursor,
    load_compositor_cursor_from_environment,
};
use oblivion_one::native::kms::{AtomicDiscovery, AtomicKmsError, KmsBackendKind};
use std::sync::Arc;

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

enum NativeKmsStartupPlan {
    Atomic {
        discovery: Box<AtomicDiscovery>,
    },
    Legacy {
        atomic_fallback_reason: Option<AtomicKmsError>,
    },
}

fn build_native_kms_startup_plan(
    policy: KmsPolicy,
    scanout: NativeScanoutKind,
    discovery: Result<AtomicDiscovery, AtomicKmsError>,
) -> NativeResult<NativeKmsStartupPlan> {
    let decision = decide_native_kms_startup(
        policy,
        scanout,
        discovery.as_ref().map(|_| ()).map_err(Clone::clone),
    )?;
    match decision {
        NativeKmsStartupDecision::Atomic => match discovery {
            Ok(discovery) => Ok(NativeKmsStartupPlan::Atomic {
                discovery: Box::new(discovery),
            }),
            Err(error) => Err(error.into()),
        },
        NativeKmsStartupDecision::Legacy {
            atomic_fallback_reason,
        } => Ok(NativeKmsStartupPlan::Legacy {
            atomic_fallback_reason,
        }),
    }
}

struct NativeRuntimeBootstrapTail {
    server: OwnCompositorServer,
    cursor_image: Arc<CompositorCursorImage>,
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
    direct_scanout_preference: NativeDirectScanoutPreference,
    pre_kms_atomic_cursor: Option<NativeAtomicCursor>,
    pre_kms_legacy_cursor: Option<NativeLegacyHardwareCursor>,
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
            cursor_image,
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
            direct_scanout_preference,
            pre_kms_atomic_cursor,
            pre_kms_legacy_cursor,
            mut cursor_render_mode,
            input_plan,
            seat_session,
            startup_app,
            effective_app_gpu_policy,
        } = parts;
        let atomic_cursor = pre_kms_atomic_cursor;
        let mut legacy_cursor = pre_kms_legacy_cursor;
        scanout.finish_initial_scanout();
        if let Some(cursor) = legacy_cursor.as_mut() {
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
                legacy_cursor = None;
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
                scanout.present(&kms_backend, None)?;
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
        } else if let Some(cursor) = atomic_cursor.as_ref() {
            println!(
                "atomic cursor: plane selected id={} size={}x{} format=0x{:08x} modifier={}",
                cursor.plane.plane_id,
                cursor.current().width,
                cursor.current().height,
                cursor.plane.format_modifier.fourcc,
                cursor.plane.format_modifier.modifier,
            );
            println!(
                "native cursor backend active: atomic hardware ({}x{})",
                cursor.current().width,
                cursor.current().height
            );
            perf.log("native.cursor", || {
                vec![
                    NativePerfField::str("backend", "hardware"),
                    NativePerfField::str("implementation", "atomic-plane"),
                    NativePerfField::str("policy", cursor_preference.as_str()),
                    NativePerfField::u64("width", u64::from(cursor.current().width)),
                    NativePerfField::u64("height", u64::from(cursor.current().height)),
                ]
            });
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
        let mut xwayland = XwaylandService::bootstrap()?;
        let mut xwayland_reactor_tokens = Vec::new();
        register_xwayland_reactor_sources(
            &mut event_loop,
            &xwayland,
            &mut xwayland_reactor_tokens,
        )?;
        if xwayland.is_eager() {
            xwayland.handle_listener_readiness(&mut process_supervisor)?;
            register_xwayland_reactor_sources(
                &mut event_loop,
                &xwayland,
                &mut xwayland_reactor_tokens,
            )?;
        }
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
        let refresh_interval = Duration::from_nanos(refresh_interval_ns);
        let triple_buffer_policy = AdaptiveTripleBufferPolicy::parse(
            std::env::var("OBLIVION_ONE_TRIPLE_BUFFERING")
                .as_deref()
                .unwrap_or("auto"),
        )
        .map_err(io::Error::other)?;
        let presentation_deadline = PresentationDeadlinePlanner::new(refresh_interval);
        let scheduled_presentation_target = None;
        let render_journal = AdaptiveRenderJournal::default();
        let adaptive_buffering = AdaptiveBufferingController::new(triple_buffer_policy);
        if let NativeScanoutBackend::AtomicEglGbm(explicit) = &scanout {
            perf.log("native.explicit_output", || {
                vec![
                    NativePerfField::str("scanout_backend", "atomic-egl-gbm-explicit"),
                    NativePerfField::str("output_swapchain", "explicit-atomic-egl-gbm"),
                    NativePerfField::u64("slot_capacity", 3),
                    NativePerfField::str("kms_backend", "atomic"),
                    NativePerfField::bool("surfaceless", true),
                    NativePerfField::str(
                        "format",
                        String::from_utf8_lossy(&explicit.format_modifier.fourcc.to_le_bytes())
                            .to_string(),
                    ),
                    NativePerfField::str(
                        "modifier",
                        format!("{:#x}", explicit.format_modifier.modifier),
                    ),
                    NativePerfField::u64(
                        "plane_count",
                        u64::from(explicit.plane_count().unwrap_or(0)),
                    ),
                    NativePerfField::str("triple_policy", triple_buffer_policy.as_str()),
                    NativePerfField::str("pacing_policy", "deadline-driven"),
                    NativePerfField::str("presentation_clock", "clock-monotonic"),
                    NativePerfField::str("render_journal", "ewma+upper-deviation+p90"),
                ]
            });
        }
        if let Some(token) = scanout.pending_page_flip_token() {
            frame_scheduler
                .note_async_submission(token, scheduler_anchor_ns)
                .map_err(io::Error::other)?;
        }
        event_loop.arm_deadline(earliest_native_deadline(
            earliest_native_deadline(
                frame_scheduler.next_deadline_ns(),
                acquire_watches.next_fallback_deadline_ns(),
            ),
            xwayland.next_deadline_ns(),
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
        let frame_pacing = NativeFramePacing::from_env();
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
        let mut runtime = Self {
            server,
            cursor_image,
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
            direct_scanout_preference,
            cursor_render_mode,
            atomic_cursor,
            legacy_cursor,
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
            xwayland,
            xwayland_reactor_tokens,
            xwayland_client_identity: None,
            drm_reactor_token: Some(drm_reactor_token),
            output_render_fence_token: None,
            frame_scheduler,
            atomic_commit_arbiter: AtomicCommitArbiter::new(),
            presentation_deadline,
            scheduled_presentation_target,
            render_journal,
            adaptive_buffering,
            triple_buffer_policy,
            pending_proven_deadline_miss: None,
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
            frame_pacing,
            last_acquire_ready_at_ns,
            resize_perf,
            pointer_constraint_backend,
            process_supervisor,
            astrea_launch_tracker: AstreaLaunchLifecycleTracker::default(),
            shutdown: NativeShutdownLifecycle::new(),
        };
        runtime.attach_xwayland_private_client()?;
        Ok(runtime)
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
        let cursor_image = Arc::new(load_compositor_cursor_from_environment());
        install_shared_compositor_cursor(cursor_image.clone());
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
        let connector_id = ConnectorId::new(target.connector_id)
            .ok_or_else(|| io::Error::other("selected connector ID is zero"))?;
        let crtc_id = CrtcId::new(target.crtc_id)
            .ok_or_else(|| io::Error::other("selected CRTC ID is zero"))?;
        let (startup_plan, mut scanout) = if scanout_plan.primary
            == NativeScanoutKind::AtomicEglGbmExplicit
        {
            if kms_policy == KmsPolicy::Legacy
                && scanout_preference != NativeScanoutPreference::Auto
            {
                return Err(
                    io::Error::other("explicit Atomic scanout cannot use Legacy KMS").into(),
                );
            }
            let discovery_result = if kms_policy == KmsPolicy::Legacy {
                Err(AtomicKmsError::new(
                    AtomicKmsErrorKind::Unsupported,
                    "Atomic discovery skipped for Legacy KMS",
                ))
            } else {
                KmsBackendSelection::discover_atomic_pipeline(
                    kms.file().as_fd(),
                    connector_id,
                    crtc_id,
                    u32::from_le_bytes(*b"XR24"),
                )
                .or_else(|error| {
                    if error.kind != AtomicKmsErrorKind::NoCompatiblePrimaryPlane {
                        return Err(error);
                    }
                    KmsBackendSelection::discover_atomic_pipeline(
                        kms.file().as_fd(),
                        connector_id,
                        crtc_id,
                        u32::from_le_bytes(*b"AR24"),
                    )
                })
            };
            match discovery_result {
                Ok(discovery) => {
                    if !discovery.optional.in_fence_fd {
                        return Err(io::Error::other(
                            "explicit Atomic EGL/GBM requires primary-plane IN_FENCE_FD",
                        )
                        .into());
                    }
                    let explicit = AtomicEglGbmScanout::create_unattached_pool(
                        kms.file(),
                        &discovery,
                        target.width,
                        target.height,
                        drm_file_generation,
                    )?;
                    let startup_plan = build_native_kms_startup_plan(
                        kms_policy,
                        NativeScanoutKind::AtomicEglGbmExplicit,
                        Ok(discovery),
                    )?;
                    (
                        startup_plan,
                        NativeScanoutBackend::from_atomic_explicit(explicit),
                    )
                }
                Err(error)
                    if kms_policy == KmsPolicy::Legacy
                        || (kms_policy == KmsPolicy::Auto
                            && scanout_preference == NativeScanoutPreference::Auto) =>
                {
                    eprintln!(
                        "native KMS: explicit Atomic discovery failed ({error}); trying compatibility scanout"
                    );
                    let fallback_plan =
                        scanout_plan.after_failed(NativeScanoutKind::AtomicEglGbmExplicit);
                    let scanout = NativeScanoutBackend::open(
                        fallback_plan,
                        kms.file(),
                        target.width,
                        target.height,
                        drm_file_generation,
                    )?;
                    let discovery = if kms_policy == KmsPolicy::Legacy {
                        Err(AtomicKmsError::new(
                            AtomicKmsErrorKind::Unsupported,
                            "Atomic discovery skipped for Legacy KMS",
                        ))
                    } else {
                        KmsBackendSelection::discover_atomic_pipeline(
                            kms.file().as_fd(),
                            connector_id,
                            crtc_id,
                            scanout.scanout_format(),
                        )
                    };
                    let startup_plan =
                        build_native_kms_startup_plan(kms_policy, scanout.kind(), discovery)?;
                    (startup_plan, scanout)
                }
                Err(error) => return Err(error.into()),
            }
        } else {
            let scanout = NativeScanoutBackend::open(
                scanout_plan.clone(),
                kms.file(),
                target.width,
                target.height,
                drm_file_generation,
            )?;
            let discovery = if kms_policy == KmsPolicy::Legacy {
                Err(AtomicKmsError::new(
                    AtomicKmsErrorKind::Unsupported,
                    "Atomic discovery skipped for Legacy KMS",
                ))
            } else {
                KmsBackendSelection::discover_atomic_pipeline(
                    kms.file().as_fd(),
                    connector_id,
                    crtc_id,
                    scanout.scanout_format(),
                )
            };
            let startup_plan =
                build_native_kms_startup_plan(kms_policy, scanout.kind(), discovery)?;
            (startup_plan, scanout)
        };
        let atomic_discovery = match &startup_plan {
            NativeKmsStartupPlan::Atomic { discovery } => Some(discovery.as_ref()),
            NativeKmsStartupPlan::Legacy { .. } => None,
        };
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
        let direct_scanout_preference = NativeDirectScanoutPreference::from_env();
        println!(
            "native cursor backend target: {}",
            cursor_preference.as_str()
        );
        let mut pre_kms_atomic_cursor = None;
        let mut pre_kms_legacy_cursor = None;
        if cursor_preference != NativeCursorPreference::Software {
            if let Some(discovery) = atomic_discovery.as_ref() {
                if let Some(plane) = discovery.cursor_plane.clone() {
                    match NativeAtomicCursor::create(
                        kms.file(),
                        plane,
                        discovery.cursor_width,
                        discovery.cursor_height,
                        drm_file_generation,
                        cursor_image.clone(),
                    ) {
                        Ok(mut cursor) => {
                            let (x, y) = input_state.cursor_position();
                            cursor.set_position(x, y);
                            cursor.set_visible(
                                input_state.cursor_visible()
                                    && server.client_cursor_render_state().is_none(),
                            );
                            println!(
                                "atomic cursor: framebuffer allocated id={} backing=dumb",
                                cursor.desired().framebuffer_id.unwrap_or(0)
                            );
                            pre_kms_atomic_cursor = Some(cursor);
                        }
                        Err(error) if cursor_preference == NativeCursorPreference::Hardware => {
                            return Err(error.into());
                        }
                        Err(error) => {
                            eprintln!(
                                "native cursor: Atomic hardware cursor unavailable: {error}; using software"
                            );
                            perf.log("native.cursor", || {
                                vec![
                                    NativePerfField::str("backend", "software"),
                                    NativePerfField::str("policy", cursor_preference.as_str()),
                                    NativePerfField::str("fallback", "create_failed"),
                                    NativePerfField::str("error", error.to_string()),
                                ]
                            });
                        }
                    }
                } else if cursor_preference == NativeCursorPreference::Hardware {
                    return Err(io::Error::other(
                        "Atomic hardware cursor requested but no compatible cursor plane was discovered",
                    )
                    .into());
                }
            } else if atomic_discovery.is_none() {
                match NativeLegacyHardwareCursor::create(
                    kms.file(),
                    target.crtc_id,
                    cursor_image.as_ref(),
                ) {
                    Ok(cursor) => pre_kms_legacy_cursor = Some(cursor),
                    Err(error) if cursor_preference == NativeCursorPreference::Hardware => {
                        return Err(error.into());
                    }
                    Err(error) => eprintln!(
                        "native cursor: legacy hardware cursor unavailable: {error}; using software"
                    ),
                }
            }
        }
        let effective_kms_kind = if atomic_discovery.is_some() {
            KmsBackendKind::Atomic
        } else {
            KmsBackendKind::Legacy
        };
        let cursor_owner = decide_native_cursor_owner(
            effective_kms_kind,
            cursor_preference,
            pre_kms_atomic_cursor.is_some() || pre_kms_legacy_cursor.is_some(),
        )
        .map_err(io::Error::other)?;
        debug_assert!(matches!(
            (effective_kms_kind, cursor_owner),
            (
                KmsBackendKind::Atomic,
                NativeCursorOwnerPlan::AtomicHardware
            ) | (
                KmsBackendKind::Legacy,
                NativeCursorOwnerPlan::LegacyHardware
            ) | (_, NativeCursorOwnerPlan::Software)
        ));
        let cursor_render_mode =
            if pre_kms_atomic_cursor.is_some() || pre_kms_legacy_cursor.is_some() {
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
        let initial_cursor_state = pre_kms_atomic_cursor.as_ref().and_then(|cursor| {
            effective_atomic_cursor_state(
                cursor,
                cursor_render_mode,
                input_state.cursor_visible(),
                server.client_cursor_render_state().is_some(),
            )
            .kms_state()
            .cloned()
        });
        let mut initial_atomic_parts = None;
        let mut initial_surface_damage = None;
        let initial_paint = if let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut scanout {
            let slot = explicit.initial_slot();
            let framebuffer = explicit.framebuffer(slot)?;
            KmsBackendSelection::test_atomic_modeset_from_discovery_with_cursor(
                kms.file().as_fd(),
                atomic_discovery
                    .as_ref()
                    .expect("explicit scanout retains Atomic discovery"),
                target.mode,
                target.width,
                target.height,
                framebuffer,
                initial_cursor_state.as_ref(),
            )?;
            initial_surface_damage = Some(server.capture_surface_damage_presentation());
            let mut initial_gpu_sampling_started = false;
            let parts = explicit.render_to_slot(
                slot,
                &mut frame_renderer,
                &server,
                &input_state,
                cursor_render_mode,
                &initial_damage,
                &mut initial_gpu_sampling_started,
            )?;
            let stats = parts.paint_stats(
                explicit.format_modifier.fourcc,
                target.width,
                target.height,
            );
            initial_atomic_parts = Some(parts);
            NativePaintOutcome::Rendered(stats)
        } else {
            match scanout.paint_server_frame(
        &mut frame_renderer,
        &server,
        &input_state,
        cursor_render_mode,
        &initial_damage,
    ) {
        Ok(paint) => paint,
        Err(error)
            if scanout.kind() == NativeScanoutKind::NativeEglGbmOpaqueCompatibility
                && scanout_preference
                    != NativeScanoutPreference::NativeEglGbmOpaqueCompatibility
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
                    NativePerfField::str(
                        "failed",
                        NativeScanoutKind::NativeEglGbmOpaqueCompatibility.as_str(),
                    ),
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
        let kms_backend = match startup_plan {
            NativeKmsStartupPlan::Atomic { discovery } => {
                if let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut scanout {
                    let mut parts = initial_atomic_parts
                        .take()
                        .expect("explicit initial render produced frame ownership");
                    let initial_framebuffer = explicit.framebuffer(parts.slot)?;
                    let submission_fence = parts.render_fence.take_submission_fd()?;
                    let backend =
                        KmsBackendSelection::initialize_atomic_from_discovery_with_fence_and_cursor(
                            kms.file().as_fd(),
                            kms_policy,
                            *discovery,
                            target.mode,
                            target.width,
                            target.height,
                            initial_framebuffer,
                            submission_fence,
                            initial_cursor_state.as_ref(),
                        )?;
                    explicit.promote_initial_presented(parts.slot, parts.scene_commit)?;
                    if let Some(surface_damage) = initial_surface_damage.take() {
                        server.commit_surface_damage_presented(surface_damage);
                    }
                    backend
                } else {
                    let initial_framebuffer =
                        FramebufferId::new(scanout.fb_id()).ok_or_else(|| {
                            io::Error::other("initial scanout framebuffer ID is zero")
                        })?;
                    KmsBackendSelection::initialize_atomic_from_discovery_with_cursor(
                        kms.file().as_fd(),
                        kms_policy,
                        *discovery,
                        target.mode,
                        target.width,
                        target.height,
                        initial_framebuffer,
                        initial_cursor_state.as_ref(),
                    )?
                }
            }
            NativeKmsStartupPlan::Legacy {
                atomic_fallback_reason,
            } => {
                let initial_framebuffer = FramebufferId::new(scanout.fb_id())
                    .ok_or_else(|| io::Error::other("initial scanout framebuffer ID is zero"))?;
                KmsBackendSelection::initialize_legacy(
                    kms.file().as_fd(),
                    kms_policy,
                    atomic_fallback_reason,
                    connector_id,
                    crtc_id,
                    target.mode,
                    initial_framebuffer,
                )?
            }
        };
        if let Some(cursor) = pre_kms_atomic_cursor.as_mut() {
            cursor.mark_initial_submitted(initial_cursor_state.as_ref());
        }
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
            cursor_image,
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
            direct_scanout_preference,
            pre_kms_atomic_cursor,
            pre_kms_legacy_cursor,
            cursor_render_mode,
            input_plan,
            seat_session,
            startup_app,
            effective_app_gpu_policy,
        })
    }
}

fn register_xwayland_reactor_sources(
    event_loop: &mut NativeEventLoop,
    service: &XwaylandService,
    tokens: &mut Vec<(ReactorToken, XwaylandReactorRegistration)>,
) -> NativeResult<()> {
    let desired: Vec<_> = service.reactor_registrations().collect();
    let mut retained = Vec::new();
    for (token, registration) in tokens.drain(..) {
        if desired.contains(&registration) {
            retained.push((token, registration));
        } else {
            event_loop.unregister(token)?;
        }
    }
    *tokens = retained;
    for registration in desired {
        if tokens.iter().any(|(_, current)| *current == registration) {
            continue;
        }
        let source = match registration.purpose {
            XwaylandReactorPurpose::ListenFilesystem | XwaylandReactorPurpose::ListenAbstract => {
                NativeEventSource::XwaylandListen
            }
            XwaylandReactorPurpose::DisplayReady => NativeEventSource::XwaylandDisplayReady,
            XwaylandReactorPurpose::Xwm => NativeEventSource::XwaylandXwm,
        };
        let token = event_loop.register(registration.fd, source)?;
        tokens.push((token, registration));
    }
    Ok(())
}
