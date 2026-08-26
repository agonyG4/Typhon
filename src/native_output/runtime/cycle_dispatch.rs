use super::cursor_cycle::apply_cursor_position;
use super::*;

use oblivion_one::control::{
    ControlCommand, ControlError, ControlErrorCode, ControlRequest, ControlResponse,
};
use oblivion_one::control_snapshots::{
    ActiveWindowSnapshot, ControlStatusSnapshot, DecorationThemeListSnapshot,
    DecorationThemeSnapshot, DoctorCheck, DoctorSeverity, DoctorSnapshot, FeatureState,
    FeatureStateSnapshot, ModeSnapshot, OutputListSnapshot, OutputSnapshot, PositionSnapshot,
    StatusSnapshot, VersionSnapshot, XwaylandStatusSnapshot,
};
use oblivion_one::cursor_manager::{
    CursorIoError, CursorIoOperation, CursorIoSubmitError, CursorJobId, CursorMutationKind,
};
use oblivion_one::native::event_loop::NativeWakeup;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyCursorArgs {}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CursorThemeArgs {
    theme: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CursorSizeArgs {
    size_px: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CursorSetArgs {
    theme: String,
    size_px: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DecorationThemeArgs {
    theme: String,
}

impl NativeRuntime {
    pub(super) fn service_control_events(&mut self, wakeup: &NativeWakeup) -> NativeResult<()> {
        if !wakeup.reasons.control() {
            return Ok(());
        }
        let pending = self.control_server.service_events(
            &mut self.event_loop,
            &wakeup.control_events,
            oblivion_one::native::control::MAX_CONTROL_OPERATIONS_PER_CYCLE,
        )?;
        for (token, request) in pending {
            if let Some(response) = self.dispatch_control_command(token, request) {
                self.control_server
                    .queue_response(&mut self.event_loop, token, response)?;
            }
        }
        Ok(())
    }

    pub(super) fn service_cursor_io_completions(
        &mut self,
        wakeup: &NativeWakeup,
    ) -> NativeResult<()> {
        let worker_failed_without_readiness = self
            .cursor_io_worker
            .as_ref()
            .is_some_and(|worker| !worker.is_available());
        if !wakeup.reasons.cursor_io_worker() && !worker_failed_without_readiness {
            return Ok(());
        }
        let terminal_readiness = wakeup.cursor_io_events.iter().any(|event| {
            event.flags & (libc::EPOLLERR | libc::EPOLLHUP | libc::EPOLLRDHUP) as u32 != 0
        });
        let (notification_error, completion, worker_unavailable) = {
            let Some(worker) = self.cursor_io_worker.as_ref() else {
                return Ok(());
            };
            let notification_error = worker.drain_notification().err();
            let completion = worker.try_completion();
            (notification_error, completion, !worker.is_available())
        };
        if notification_error.is_some() {
            self.cursor_manager.note_worker_notification_failure();
        }
        let mut terminal_failure =
            terminal_readiness || worker_unavailable || notification_error.is_some();
        if let Some(completion) = completion {
            let Some(pending) = self.pending_cursor_job.take() else {
                self.cursor_manager.note_stale_client_completion();
                if terminal_failure {
                    self.disable_cursor_io_worker();
                }
                return Ok(());
            };
            if completion.job_id != pending.job_id {
                self.cursor_manager.note_stale_client_completion();
            } else {
                let response = match completion.result {
                    Ok(prepared) => {
                        let change = self.cursor_manager.publish_prepared(prepared);
                        self.publish_cursor_change(change);
                        cursor_snapshot_response(self, pending.request_id)
                    }
                    Err(error) => {
                        terminal_failure |= matches!(
                            error,
                            CursorIoError::WorkerPanicked | CursorIoError::WorkerUnavailable
                        );
                        self.cursor_manager.note_worker_error(error);
                        cursor_failure(pending.request_id, map_cursor_io_error(error))
                    }
                };
                if !self.control_server.has_client(pending.token) {
                    self.cursor_manager.note_stale_client_completion();
                }
                self.control_server.queue_response(
                    &mut self.event_loop,
                    pending.token,
                    response,
                )?;
            }
        } else if terminal_failure && let Some(pending) = self.pending_cursor_job.take() {
            self.cursor_manager
                .note_worker_error(CursorIoError::WorkerUnavailable);
            let response = ControlResponse::failure(
                pending.request_id,
                ControlError::new(ControlErrorCode::Internal, "cursor I/O worker unavailable")
                    .with_detail("cursor_io_unavailable"),
            );
            if !self.control_server.has_client(pending.token) {
                self.cursor_manager.note_stale_client_completion();
            }
            self.control_server
                .queue_response(&mut self.event_loop, pending.token, response)?;
        }
        if terminal_failure {
            self.disable_cursor_io_worker();
        }
        Ok(())
    }

    fn disable_cursor_io_worker(&mut self) {
        if let Some(token) = self.cursor_io_worker_reactor_token.take() {
            let _ = self.event_loop.unregister(token);
        }
        self.cursor_io_worker.take();
    }

    fn dispatch_control_command(
        &mut self,
        token: oblivion_one::native::event_loop::ReactorToken,
        request: ControlRequest,
    ) -> Option<ControlResponse> {
        let Some(command) = ControlCommand::parse(&request.command) else {
            return Some(ControlResponse::failure(
                request.id,
                ControlError::new(ControlErrorCode::InvalidCommand, "unknown control command"),
            ));
        };
        let result = match command {
            ControlCommand::Version => serde_json::to_value(VersionSnapshot {
                protocol_version: oblivion_one::control::CONTROL_VERSION,
                compositor_name: "Typhon".to_string(),
                compositor_version: env!("CARGO_PKG_VERSION").to_string(),
                git_commit: option_env!("GIT_COMMIT").map(str::to_string),
                build_profile: if cfg!(debug_assertions) {
                    "debug".to_string()
                } else {
                    "release".to_string()
                },
                rustc_version: option_env!("RUSTC_VERSION").map(str::to_string),
            }),
            ControlCommand::Status => {
                let (_, mapped, minimized) = self.server.control_window_counts();
                serde_json::to_value(StatusSnapshot {
                    instance: self.server.socket_name().to_string(),
                    wayland_display: self.server.socket_name().to_string(),
                    uptime_ms: self
                        .started_at
                        .elapsed()
                        .as_millis()
                        .min(u128::from(u64::MAX)) as u64,
                    session_state: self.session.control_state(),
                    shutdown_state: self.shutdown.state().as_str().to_string(),
                    output_count: if self.scanout_destroyed { 0 } else { 1 },
                    mapped_window_count: mapped,
                    minimized_window_count: minimized,
                    active_window: self
                        .server
                        .control_active_window_snapshot()
                        .map(|window| window.id),
                    xwayland: XwaylandStatusSnapshot {
                        configured: !matches!(
                            self.xwayland.state_kind(),
                            oblivion_one::xwayland::XwaylandStateKind::Disabled
                        ),
                        state: self.xwayland.state_kind().as_str().to_string(),
                        generation: self
                            .xwayland
                            .generation()
                            .map(|generation| generation.get()),
                    },
                    control: ControlStatusSnapshot {
                        endpoint_active: !self.shutdown.is_complete(),
                        client_count: u32::try_from(self.control_server.client_count())
                            .unwrap_or(u32::MAX),
                        accepted: self.control_server.counters().accepted,
                    },
                })
            }
            ControlCommand::Performance => serde_json::to_value(self.performance_snapshot()),
            ControlCommand::Doctor => {
                let output_available = !self.scanout_destroyed;
                let session_severity = self.session.doctor_severity();
                let direct_state = self.direct_scanout_state();
                let checks = vec![
                    doctor_check(
                        "control.endpoint",
                        DoctorSeverity::Ok,
                        "secure endpoint active",
                    ),
                    doctor_check(
                        "session.state",
                        session_severity,
                        format!("session {}", self.session.state_name()),
                    ),
                    doctor_check(
                        "output.available",
                        if output_available {
                            DoctorSeverity::Ok
                        } else {
                            DoctorSeverity::Error
                        },
                        if output_available {
                            "active output available"
                        } else {
                            "no active output"
                        },
                    ),
                    doctor_check(
                        "kms.backend",
                        DoctorSeverity::Ok,
                        self.kms_backend.effective_kind().as_str(),
                    ),
                    doctor_check(
                        "renderer.backend",
                        DoctorSeverity::Ok,
                        self.scanout.kind().as_str(),
                    ),
                    doctor_check(
                        "output.mode",
                        if output_available {
                            DoctorSeverity::Ok
                        } else {
                            DoctorSeverity::Error
                        },
                        self.mode_label.clone(),
                    ),
                    doctor_check(
                        "cursor.backend",
                        DoctorSeverity::Ok,
                        self.cursor_render_mode.as_str(),
                    ),
                    doctor_check(
                        "cursor.configuration",
                        self.cursor_configuration_doctor_severity(),
                        format!(
                            "desired={}@{} active={}@{} source={} persistence={} asset={}",
                            self.cursor_manager.desired_configuration().theme,
                            self.cursor_manager.desired_configuration().size_px,
                            self.cursor_manager.active_configuration().theme,
                            self.cursor_manager.active_configuration().size_px,
                            self.cursor_manager.source().as_str(),
                            self.cursor_manager.persistence().as_str(),
                            self.cursor_manager.asset_source().as_str(),
                        ),
                    ),
                    doctor_check(
                        "xwayland.state",
                        match self.xwayland.state_kind() {
                            oblivion_one::xwayland::XwaylandStateKind::Failed => {
                                DoctorSeverity::Warning
                            }
                            oblivion_one::xwayland::XwaylandStateKind::Disabled => {
                                DoctorSeverity::Ok
                            }
                            _ => DoctorSeverity::Ok,
                        },
                        format!("XWayland {}", self.xwayland.state_kind().as_str()),
                    ),
                    doctor_check(
                        "shutdown.state",
                        if self.shutdown.is_complete() {
                            DoctorSeverity::Warning
                        } else {
                            DoctorSeverity::Ok
                        },
                        self.shutdown.state().as_str(),
                    ),
                    doctor_check(
                        "kms_worker.state",
                        kms_worker_doctor_severity(
                            self.kms_commit_worker_policy,
                            self.kms_commit_worker_transport,
                            self.kms_commit_worker_startup,
                            self.kms_commit_worker.is_some(),
                        ),
                        format!(
                            "policy={} transport={} startup={}",
                            self.kms_commit_worker_policy.as_str(),
                            self.kms_commit_worker_transport.as_str(),
                            self.kms_commit_worker_startup.as_str()
                        ),
                    ),
                    doctor_check(
                        "direct_scanout.state",
                        direct_scanout_doctor_severity(
                            self.direct_scanout_preference.enabled(),
                            direct_state,
                        ),
                        direct_state.as_str(),
                    ),
                    doctor_check(
                        "triple_buffering.state",
                        oblivion_one::native::adaptive_buffering::triple_buffering_doctor_severity(
                            self.triple_buffer_policy,
                            self.adaptive_buffering.mode(),
                            self.adaptive_buffering
                                .force_unavailable_blocker()
                                .is_some(),
                        ),
                        format!(
                            "policy={} mode={}",
                            self.triple_buffer_policy.as_str(),
                            self.adaptive_buffering.mode().as_str()
                        ),
                    ),
                    doctor_check(
                        "vrr.state",
                        vrr_doctor_severity(self.vrr_plan.requested, self.vrr_plan.supported),
                        format!(
                            "requested={} supported={}",
                            self.vrr_plan.requested.as_str(),
                            self.vrr_plan.supported
                        ),
                    ),
                ];
                serde_json::to_value(DoctorSnapshot {
                    healthy: checks
                        .iter()
                        .all(|check| matches!(check.severity, DoctorSeverity::Ok)),
                    checks,
                })
            }
            ControlCommand::Outputs => serde_json::to_value(OutputListSnapshot {
                outputs: if self.scanout_destroyed {
                    Vec::new()
                } else {
                    vec![self.control_output_snapshot()]
                },
                total: if self.scanout_destroyed { 0 } else { 1 },
                truncated: false,
            }),
            ControlCommand::Windows => self
                .server
                .control_window_list_snapshot()
                .and_then(serde_json::to_value),
            ControlCommand::ActiveWindow => serde_json::to_value(ActiveWindowSnapshot {
                window: self.server.control_active_window_snapshot(),
            }),
            ControlCommand::CursorGet => {
                if serde_json::from_value::<EmptyCursorArgs>(request.args).is_err() {
                    self.cursor_manager.note_validation_failure();
                    return Some(cursor_argument_failure(request.id));
                }
                self.cursor_manager.note_get();
                serde_json::to_value(self.cursor_snapshot())
            }
            ControlCommand::CursorSetTheme => {
                let args = match serde_json::from_value::<CursorThemeArgs>(request.args) {
                    Ok(args) => args,
                    Err(_) => {
                        self.cursor_manager.note_validation_failure();
                        return Some(cursor_argument_failure(request.id));
                    }
                };
                let configuration = match self.cursor_manager.configuration_for_theme(&args.theme) {
                    Ok(configuration) => configuration,
                    Err(error) => return Some(cursor_failure(request.id, error)),
                };
                return self.queue_cursor_operation(
                    token,
                    request.id,
                    CursorIoOperation::Apply {
                        job_id: CursorJobId(0),
                        configuration,
                        persist: true,
                        kind: CursorMutationKind::Theme,
                    },
                );
            }
            ControlCommand::CursorSetSize => {
                let args = match serde_json::from_value::<CursorSizeArgs>(request.args) {
                    Ok(args) => args,
                    Err(_) => {
                        self.cursor_manager.note_validation_failure();
                        return Some(cursor_argument_failure(request.id));
                    }
                };
                let configuration = match self.cursor_manager.configuration_for_size(args.size_px) {
                    Ok(configuration) => configuration,
                    Err(error) => return Some(cursor_failure(request.id, error)),
                };
                return self.queue_cursor_operation(
                    token,
                    request.id,
                    CursorIoOperation::Apply {
                        job_id: CursorJobId(0),
                        configuration,
                        persist: true,
                        kind: CursorMutationKind::Size,
                    },
                );
            }
            ControlCommand::CursorSet => {
                let args = match serde_json::from_value::<CursorSetArgs>(request.args) {
                    Ok(args) => args,
                    Err(_) => {
                        self.cursor_manager.note_validation_failure();
                        return Some(cursor_argument_failure(request.id));
                    }
                };
                let configuration = match self
                    .cursor_manager
                    .configuration_for_values(&args.theme, args.size_px)
                {
                    Ok(configuration) => configuration,
                    Err(error) => return Some(cursor_failure(request.id, error)),
                };
                return self.queue_cursor_operation(
                    token,
                    request.id,
                    CursorIoOperation::Apply {
                        job_id: CursorJobId(0),
                        configuration,
                        persist: true,
                        kind: CursorMutationKind::Combined,
                    },
                );
            }
            ControlCommand::CursorReload => {
                if serde_json::from_value::<EmptyCursorArgs>(request.args).is_err() {
                    self.cursor_manager.note_validation_failure();
                    return Some(cursor_argument_failure(request.id));
                }
                return self.queue_cursor_operation(
                    token,
                    request.id,
                    CursorIoOperation::Reload {
                        job_id: CursorJobId(0),
                    },
                );
            }
            ControlCommand::DecorationStatus => {
                let (selected_theme, active_theme, schema_version, generation, source, last_error) =
                    self.server.decoration_theme_status();
                serde_json::to_value(DecorationThemeSnapshot {
                    selected_theme,
                    active_theme,
                    schema_version,
                    generation,
                    source,
                    last_error,
                })
            }
            ControlCommand::DecorationList => {
                let (selected_theme, ..) = self.server.decoration_theme_status();
                serde_json::to_value(DecorationThemeListSnapshot {
                    themes: self.server.decoration_theme_list(),
                    selected_theme,
                })
            }
            ControlCommand::DecorationSetTheme => {
                let args = match serde_json::from_value::<DecorationThemeArgs>(request.args) {
                    Ok(args) => args,
                    Err(_) => {
                        return Some(ControlResponse::failure(
                            request.id,
                            ControlError::new(
                                ControlErrorCode::InvalidArgument,
                                "decoration set-theme requires a theme",
                            ),
                        ));
                    }
                };
                if let Err(error) = self.server.set_decoration_theme(&args.theme) {
                    return Some(ControlResponse::failure(
                        request.id,
                        ControlError::new(ControlErrorCode::InvalidArgument, error),
                    ));
                }
                let (selected_theme, active_theme, schema_version, generation, source, last_error) =
                    self.server.decoration_theme_status();
                serde_json::to_value(DecorationThemeSnapshot {
                    selected_theme,
                    active_theme,
                    schema_version,
                    generation,
                    source,
                    last_error,
                })
            }
            ControlCommand::DecorationReload => {
                if serde_json::from_value::<EmptyCursorArgs>(request.args).is_err() {
                    return Some(ControlResponse::failure(
                        request.id,
                        ControlError::new(
                            ControlErrorCode::InvalidArgument,
                            "decoration reload takes no arguments",
                        ),
                    ));
                }
                if let Err(error) = self.server.reload_decoration_theme() {
                    return Some(ControlResponse::failure(
                        request.id,
                        ControlError::new(ControlErrorCode::Internal, error),
                    ));
                }
                let (selected_theme, active_theme, schema_version, generation, source, last_error) =
                    self.server.decoration_theme_status();
                serde_json::to_value(DecorationThemeSnapshot {
                    selected_theme,
                    active_theme,
                    schema_version,
                    generation,
                    source,
                    last_error,
                })
            }
            _ => {
                return Some(ControlResponse::failure(
                    request.id,
                    ControlError::new(
                        ControlErrorCode::InvalidCommand,
                        "command is not available in M3",
                    ),
                ));
            }
        };
        match result {
            Ok(result) => Some(ControlResponse::success(request.id, result)),
            Err(_) => Some(ControlResponse::failure(
                request.id,
                ControlError::new(ControlErrorCode::Internal, "control snapshot failed"),
            )),
        }
    }

    fn queue_cursor_operation(
        &mut self,
        token: oblivion_one::native::event_loop::ReactorToken,
        request_id: u64,
        operation: CursorIoOperation,
    ) -> Option<ControlResponse> {
        if self.pending_cursor_job.is_some() {
            self.cursor_manager.note_cursor_job_busy();
            return Some(cursor_failure(
                request_id,
                oblivion_one::cursor_manager::CursorManagerError::ResourceBusy,
            ));
        }
        if let CursorIoOperation::Apply { configuration, .. } = &operation
            && self.cursor_manager.is_no_op(configuration)
        {
            self.cursor_manager.note_no_op();
            return Some(cursor_snapshot_response(self, request_id));
        }
        if let Err(error) = self.cursor_manager.ensure_mutation_capacity() {
            self.cursor_manager.note_cursor_job_busy();
            return Some(cursor_failure(request_id, error));
        }
        let Some(worker) = self.cursor_io_worker.as_ref() else {
            self.cursor_manager.note_worker_unavailable();
            return Some(ControlResponse::failure(
                request_id,
                ControlError::new(ControlErrorCode::Internal, "cursor I/O worker unavailable")
                    .with_detail("cursor_io_unavailable"),
            ));
        };
        if !worker.is_available() {
            self.cursor_manager.note_worker_unavailable();
            return Some(ControlResponse::failure(
                request_id,
                ControlError::new(ControlErrorCode::Internal, "cursor I/O worker unavailable")
                    .with_detail("cursor_io_unavailable"),
            ));
        }
        let job_id = CursorJobId(self.next_cursor_job_id.max(1));
        self.next_cursor_job_id = self.next_cursor_job_id.saturating_add(1).max(1);
        let operation = match operation {
            CursorIoOperation::Apply {
                configuration,
                persist,
                kind,
                ..
            } => CursorIoOperation::Apply {
                job_id,
                configuration,
                persist,
                kind,
            },
            CursorIoOperation::Reload { .. } => CursorIoOperation::Reload { job_id },
        };
        self.pending_cursor_job = Some(PendingCursorJob {
            token,
            request_id,
            job_id,
        });
        match worker.submit(operation) {
            Ok(()) => {
                self.cursor_manager.note_cursor_job_submitted();
                None
            }
            Err(CursorIoSubmitError::Busy) => {
                self.pending_cursor_job = None;
                self.cursor_manager.note_cursor_job_busy();
                Some(cursor_failure(
                    request_id,
                    oblivion_one::cursor_manager::CursorManagerError::ResourceBusy,
                ))
            }
            Err(CursorIoSubmitError::Closed) => {
                self.pending_cursor_job = None;
                self.cursor_manager.note_worker_unavailable();
                Some(ControlResponse::failure(
                    request_id,
                    ControlError::new(ControlErrorCode::Internal, "cursor I/O worker closed")
                        .with_detail("cursor_io_unavailable"),
                ))
            }
            Err(CursorIoSubmitError::Unavailable) => {
                self.pending_cursor_job = None;
                self.cursor_manager.note_worker_unavailable();
                Some(ControlResponse::failure(
                    request_id,
                    ControlError::new(ControlErrorCode::Internal, "cursor I/O worker unavailable")
                        .with_detail("cursor_io_unavailable"),
                ))
            }
        }
    }

    fn cursor_snapshot(&self) -> oblivion_one::control_snapshots::CursorSnapshot {
        self.cursor_manager.snapshot(self.cursor_backend_snapshot())
    }

    fn cursor_backend_snapshot(&self) -> oblivion_one::control_snapshots::CursorBackendSnapshot {
        if self.scanout_destroyed {
            oblivion_one::control_snapshots::CursorBackendSnapshot::Unavailable
        } else if !self.input_state.cursor_visible() {
            oblivion_one::control_snapshots::CursorBackendSnapshot::Hidden
        } else {
            match self.cursor_render_mode {
                NativeCursorRenderMode::Hardware => {
                    oblivion_one::control_snapshots::CursorBackendSnapshot::Hardware
                }
                NativeCursorRenderMode::Software | NativeCursorRenderMode::SoftwareClient => {
                    oblivion_one::control_snapshots::CursorBackendSnapshot::Software
                }
            }
        }
    }

    fn cursor_configuration_doctor_severity(&self) -> DoctorSeverity {
        cursor_configuration_doctor_severity(
            !self.scanout_destroyed,
            true,
            true,
            self.cursor_manager.active_configuration()
                == self.cursor_manager.desired_configuration(),
            self.cursor_manager.persistence(),
            self.cursor_manager.asset_source(),
        )
    }

    fn publish_cursor_change(&mut self, change: oblivion_one::cursor_manager::CursorChange) {
        if !change.published {
            return;
        }
        self.cursor_image = if self.server.interaction_cursor_override_active() {
            self.cursor_manager
                .active_image_for_shape(self.server.compositor_cursor_shape())
        } else {
            match self.server.client_cursor_shape() {
                Some(shape) => self.cursor_manager.active_image_for_protocol_shape(shape),
                None => self
                    .cursor_manager
                    .active_image_for_shape(self.server.compositor_cursor_shape()),
            }
        };
        self.frame_renderer
            .set_cursor_image(self.cursor_image.clone());
        self.scanout.set_cursor_image(self.cursor_image.clone());
        oblivion_one::cursor_theme::install_shared_compositor_cursor(self.cursor_image.clone());
        self.queued_redraw_requested = true;
    }

    #[allow(unused_variables)]
    pub(super) fn dispatch_wayland_and_input(
        &mut self,
        cycle: &mut NativeCycleState,
        dispatch_wayland: bool,
    ) -> NativeResult<()> {
        if cycle.wakeup.reasons.input() {
            NativeSessionIo::observe(self, NativeIoOperation::RawInputAction);
        }
        let xwayland_app_environment = self.xwayland.normal_app_environment();
        let perf = self.perf;
        let Self {
            server,
            perf: _,
            kms,
            kms_backend,
            target,
            mode_label,
            refresh_hz,
            drm_file_generation,
            drm_timestamp_clock,
            presentation_clock,
            scanout,
            frame_renderer,
            input_state,
            cursor_preference,
            cursor_render_mode,
            atomic_cursor,
            legacy_cursor,
            input_devices,
            input_batch,
            acquire_notifier,
            acquire_watches,
            parked_acquire_watches: _,
            event_loop,
            drm_reactor_token: _,
            cursor_output_arbitration,
            frame_scheduler,
            effective_app_gpu_policy,
            scene_history: _,
            queued_redraw_requested,
            frame_index,
            known_toplevels,
            pending_launches,
            mismatched_pageflip_events,
            stale_pageflip_events,
            presentation_cadence: _,
            last_acquire_ready_at_ns,
            resize_perf,
            pointer_constraint_backend,
            seat_session,
            process_supervisor,
            render_telemetry,
            shutdown: _,
            session: _,
            ..
        } = self;
        let present_us = 0;
        let pageflip_pending_at_tick = scanout.page_flip_pending();
        let (accepted, tick_us) = if dispatch_wayland {
            render_telemetry
                .resource_efficiency
                .record_server_tick_call();
            let tick_start = Instant::now();
            let accepted = server.tick()?;
            render_telemetry.resource_efficiency.record_client_flush();
            (accepted, elapsed_micros(tick_start))
        } else {
            (0, 0)
        };
        let mut redraw_requested = process_native_pointer_constraint_backend_requests(
            server,
            pointer_constraint_backend,
            input_state,
            *cursor_render_mode,
        )?;
        synchronize_cursor_state_for_server(server, atomic_cursor, legacy_cursor, input_state)?;
        let current_toplevels = server.xdg_toplevels();
        if current_toplevels > *known_toplevels {
            for _ in *known_toplevels..current_toplevels {
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
            *known_toplevels = current_toplevels;
        }
        if accepted > 0 {
            println!(
                "accepted {accepted} client(s); total {}",
                server.accepted_clients()
            );
        }
        server.begin_native_input_batch();
        let mut skipped_input_repaints = 0usize;
        let input_drain_start = Instant::now();
        input_devices.drain_events_into(input_batch);
        let input_drain_us = elapsed_micros(input_drain_start);
        let raw_input_events = input_batch.raw.len();
        for _ in 0..raw_input_events {
            render_telemetry
                .resource_efficiency
                .record_raw_input_event();
        }
        let input_event_timestamp_usec = matches!(
            input_devices.kind(),
            NativeInputBackendKind::LibseatLibinputUdev
                | NativeInputBackendKind::DirectLibinputUdev
        )
        .then(|| {
            input_batch
                .raw
                .iter()
                .filter_map(|event| event.timestamp_usec())
                .max()
        })
        .flatten();
        input_batch.coalesce_pointer_motion_events();
        let coalesced_input_events = input_batch.coalesced.len();
        for _ in 0..coalesced_input_events {
            render_telemetry
                .resource_efficiency
                .record_coalesced_input_event();
        }
        for (event_index, event) in input_batch.coalesced.drain(..).enumerate() {
            let may_change_pointer_constraints = event.may_change_pointer_constraints();
            let mut effect = input_state.reconcile_keyboard_shortcut_inhibition(
                server.keyboard_shortcut_inhibition_snapshot(),
            );
            effect.append(input_state.handle_hardware_input_event(event));
            if effect.pointer_motion.is_some() || effect.relative_motion.is_some() {
                render_telemetry.resource_efficiency.record_pointer_sample();
            }
            let effect_requested_redraw = effect.redraw_requested;
            let cursor_visible = !server.client_cursor_explicitly_hidden()
                && (server.client_cursor_render_state().is_some()
                    || server.interaction_cursor_override_active()
                    || input_state.cursor_visible());
            if let Err(error) = apply_cursor_position(
                atomic_cursor,
                legacy_cursor,
                effect.cursor_position,
                cursor_visible,
                *cursor_preference,
                cursor_render_mode,
                perf,
            ) {
                if *cursor_preference == NativeCursorPreference::Hardware {
                    let shutdown_result = acquire_watches.shutdown(event_loop);
                    let _ = server.end_native_input_batch();
                    shutdown_result?;
                    return Err(error.into());
                }
                let _ = server.end_native_input_batch();
                return Err(error.into());
            }
            let application = match apply_native_input_effect(
                effect,
                NativeInputApplyContext {
                    server,
                    perf,
                    resize_perf,
                    cursor_mode: *cursor_render_mode,
                    app_gpu_policy: *effective_app_gpu_policy,
                    seat_session: seat_session.as_ref(),
                    process_supervisor,
                    xwayland: xwayland_app_environment.clone(),
                },
            ) {
                Ok(application) => application,
                Err(error) => {
                    let _ = server.end_native_input_batch();
                    return Err(error.into());
                }
            };
            if application.exit_requested {
                cycle.shutdown_requested = true;
                break;
            }
            if let Some(launch) = application.launch {
                log_native_app_spawn(perf, &launch);
                pending_launches.push_back(launch);
            }
            if effect_requested_redraw && !application.redraw_requested {
                skipped_input_repaints = skipped_input_repaints.saturating_add(1);
            }
            redraw_requested |= application.redraw_requested;
            let interaction_reconciled = reconcile_trigger_liveness(
                server,
                input_state,
                &format!("event_index={event_index}"),
            );
            redraw_requested |= interaction_reconciled;
            // A readable Wayland wake already paid for this cycle's one
            // read-side dispatch. Native-only key/button input retains the
            // narrow follow-up for pointer-constraint state, but combined
            // readiness must not turn into a duplicate full tick.
            if may_change_pointer_constraints && !dispatch_wayland {
                render_telemetry
                    .resource_efficiency
                    .record_server_tick_call();
                if let Err(error) = server.tick() {
                    let _ = server.end_native_input_batch();
                    return Err(error.into());
                }
                redraw_requested |= match process_native_pointer_constraint_backend_requests(
                    server,
                    pointer_constraint_backend,
                    input_state,
                    *cursor_render_mode,
                ) {
                    Ok(redraw_requested) => redraw_requested,
                    Err(error) => {
                        let _ = server.end_native_input_batch();
                        return Err(error);
                    }
                };
                if let Err(error) = synchronize_cursor_state_for_server(
                    server,
                    atomic_cursor,
                    legacy_cursor,
                    input_state,
                ) {
                    let _ = server.end_native_input_batch();
                    return Err(error.into());
                }
            }
        }
        let interaction_reconciled = reconcile_trigger_liveness(server, input_state, "batch_end");
        redraw_requested |= interaction_reconciled;
        redraw_requested |= match process_native_pointer_constraint_backend_requests(
            server,
            pointer_constraint_backend,
            input_state,
            *cursor_render_mode,
        ) {
            Ok(redraw_requested) => redraw_requested,
            Err(error) => {
                let _ = server.end_native_input_batch();
                return Err(error);
            }
        };
        if let Err(error) =
            synchronize_cursor_state_for_server(server, atomic_cursor, legacy_cursor, input_state)
        {
            let _ = server.end_native_input_batch();
            return Err(error.into());
        }
        let _ = observe_atomic_cursor_output_liveness(
            atomic_cursor.as_ref(),
            cursor_output_arbitration,
            frame_scheduler,
            monotonic_now_ns()?,
        );
        let client_flush = server.end_native_input_batch()?;
        if client_flush {
            render_telemetry.resource_efficiency.record_client_flush();
        }
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
        cycle.present_us = present_us;
        cycle.pageflip_pending_at_tick = pageflip_pending_at_tick;
        cycle.tick_us = tick_us;
        cycle.accepted = accepted;
        cycle.redraw_requested = redraw_requested;
        cycle.skipped_input_repaints = skipped_input_repaints;
        cycle.input_drain_us = input_drain_us;
        cycle.raw_input_events = raw_input_events;
        cycle.coalesced_input_events = coalesced_input_events;
        Ok(())
    }
}

fn cursor_failure(
    id: u64,
    error: oblivion_one::cursor_manager::CursorManagerError,
) -> ControlResponse {
    let code = if matches!(
        error,
        oblivion_one::cursor_manager::CursorManagerError::ResourceBusy
            | oblivion_one::cursor_manager::CursorManagerError::PersistenceBusy
            | oblivion_one::cursor_manager::CursorManagerError::WorkerUnavailable
    ) {
        ControlErrorCode::Internal
    } else {
        ControlErrorCode::InvalidArgument
    };
    ControlResponse::failure(
        id,
        ControlError::new(code, "cursor command failed").with_detail(error.detail()),
    )
}

fn map_cursor_io_error(error: CursorIoError) -> oblivion_one::cursor_manager::CursorManagerError {
    match error {
        CursorIoError::Load(error) => match error {
            oblivion_one::cursor_theme::CursorThemeLoadError::ThemeNotFound => {
                oblivion_one::cursor_manager::CursorManagerError::ThemeNotFound
            }
            oblivion_one::cursor_theme::CursorThemeLoadError::RequiredPointerMissing => {
                oblivion_one::cursor_manager::CursorManagerError::RequiredPointerMissing
            }
            oblivion_one::cursor_theme::CursorThemeLoadError::CursorFileReadFailed => {
                oblivion_one::cursor_manager::CursorManagerError::CursorFileReadFailed
            }
            oblivion_one::cursor_theme::CursorThemeLoadError::CursorFileInvalid
            | oblivion_one::cursor_theme::CursorThemeLoadError::CursorFileTooLarge
            | oblivion_one::cursor_theme::CursorThemeLoadError::FrameBoundsExceeded => {
                oblivion_one::cursor_manager::CursorManagerError::CursorFileInvalid
            }
        },
        CursorIoError::Persistence(error) => match error {
            oblivion_one::cursor_persistence::CursorPersistenceError::Missing => {
                oblivion_one::cursor_manager::CursorManagerError::ConfigMissing
            }
            oblivion_one::cursor_persistence::CursorPersistenceError::Invalid => {
                oblivion_one::cursor_manager::CursorManagerError::ConfigInvalid
            }
            oblivion_one::cursor_persistence::CursorPersistenceError::Insecure => {
                oblivion_one::cursor_manager::CursorManagerError::ConfigInsecure
            }
            oblivion_one::cursor_persistence::CursorPersistenceError::WriteFailed => {
                oblivion_one::cursor_manager::CursorManagerError::ConfigWriteFailed
            }
            oblivion_one::cursor_persistence::CursorPersistenceError::Busy => {
                oblivion_one::cursor_manager::CursorManagerError::PersistenceBusy
            }
        },
        CursorIoError::WorkerPanicked | CursorIoError::WorkerUnavailable => {
            oblivion_one::cursor_manager::CursorManagerError::WorkerUnavailable
        }
    }
}

fn cursor_argument_failure(id: u64) -> ControlResponse {
    ControlResponse::failure(
        id,
        ControlError::new(
            ControlErrorCode::InvalidArgument,
            "invalid cursor arguments",
        )
        .with_detail("invalid_cursor_arguments"),
    )
}

fn cursor_snapshot_response(runtime: &NativeRuntime, id: u64) -> ControlResponse {
    match serde_json::to_value(runtime.cursor_snapshot()) {
        Ok(snapshot) => ControlResponse::success(id, snapshot),
        Err(_) => ControlResponse::failure(
            id,
            ControlError::new(
                ControlErrorCode::Internal,
                "cursor snapshot serialization failed",
            )
            .with_detail("cursor_snapshot_internal"),
        ),
    }
}

fn cursor_configuration_doctor_severity(
    runtime_available: bool,
    active_theme_available: bool,
    software_fallback_available: bool,
    active_matches_desired: bool,
    persistence: oblivion_one::control_snapshots::CursorPersistenceSnapshot,
    asset_source: oblivion_one::control_snapshots::CursorAssetSource,
) -> DoctorSeverity {
    use oblivion_one::control_snapshots::{CursorAssetSource, CursorPersistenceSnapshot};

    if !runtime_available || (!active_theme_available && !software_fallback_available) {
        return DoctorSeverity::Error;
    }
    if !active_matches_desired {
        return DoctorSeverity::Warning;
    }
    if matches!(asset_source, CursorAssetSource::BuiltinFallback) {
        return DoctorSeverity::Warning;
    }
    match persistence {
        CursorPersistenceSnapshot::Invalid
        | CursorPersistenceSnapshot::Insecure
        | CursorPersistenceSnapshot::WriteFailed => DoctorSeverity::Warning,
        CursorPersistenceSnapshot::Saved | CursorPersistenceSnapshot::Missing => DoctorSeverity::Ok,
    }
}

fn doctor_check(id: &str, severity: DoctorSeverity, summary: impl Into<String>) -> DoctorCheck {
    DoctorCheck {
        id: id.to_string(),
        severity,
        summary: summary.into(),
        detail: None,
    }
}

impl NativeRuntime {
    fn direct_scanout_state(&self) -> FeatureState {
        if !self.direct_scanout_preference.enabled() || self.scanout_destroyed {
            FeatureState::Unavailable
        } else if self
            .presented_planes
            .primary
            .is_some_and(PresentedPrimaryAssignment::is_direct)
        {
            FeatureState::Active
        } else if self.direct_scanout_qualification.is_qualified() {
            FeatureState::Available
        } else {
            FeatureState::Configured
        }
    }

    fn control_output_snapshot(&self) -> OutputSnapshot {
        let direct_state = self.direct_scanout_state();
        let vrr_state = if !self.vrr_plan.supported {
            FeatureState::Unavailable
        } else if self.vrr_plan.planned_enabled {
            FeatureState::Configured
        } else {
            FeatureState::Available
        };
        OutputSnapshot {
            id: "oblivion-1".to_string(),
            name: "Oblivion-1".to_string(),
            make: None,
            model: None,
            serial: None,
            enabled: !self.scanout_destroyed,
            current_mode: (!self.scanout_destroyed).then_some(ModeSnapshot {
                width: self.target.width,
                height: self.target.height,
                refresh_millihz: self.refresh_hz.saturating_mul(1000),
            }),
            physical_size_mm: None,
            scale_milli: 1000,
            transform: "normal".to_string(),
            position: PositionSnapshot { x: 0, y: 0 },
            focused: true,
            backend: self.kms_backend.effective_kind().as_str().to_string(),
            vrr: FeatureStateSnapshot { state: vrr_state },
            direct_scanout: FeatureStateSnapshot {
                state: direct_state,
            },
        }
    }
}

fn reconcile_trigger_liveness(
    server: &mut OwnCompositorServer,
    input_state: &NativeInputState,
    after_event: &str,
) -> bool {
    let Some(snapshot) = server.window_interaction_debug_snapshot() else {
        return false;
    };
    let trigger_pressed = snapshot
        .trigger_button
        .is_none_or(|button| input_state.is_pointer_button_pressed(button));
    if let Some(trigger_button) = snapshot.trigger_button
        && !trigger_pressed
    {
        resize_debug_log(|| {
            format!(
                "event=trigger_mismatch interaction_id={} trigger_button={} physical_pressed=false pressed_buttons={:?} after_event={after_event}",
                snapshot.interaction_id,
                trigger_button,
                input_state.pressed_pointer_buttons_snapshot(),
            )
        });
    };
    server.reconcile_window_interaction_trigger(trigger_pressed)
}

#[cfg(test)]
mod cursor_doctor_tests {
    use super::cursor_configuration_doctor_severity;
    use oblivion_one::control_snapshots::{
        CursorAssetSource, CursorPersistenceSnapshot, DoctorSeverity,
    };

    #[test]
    fn cursor_configuration_doctor_matrix_preserves_healthy_fallbacks() {
        let cases = [
            (
                true,
                true,
                true,
                true,
                CursorPersistenceSnapshot::Missing,
                CursorAssetSource::SystemTheme,
                DoctorSeverity::Ok,
            ),
            (
                true,
                true,
                true,
                true,
                CursorPersistenceSnapshot::Saved,
                CursorAssetSource::SystemTheme,
                DoctorSeverity::Ok,
            ),
            (
                true,
                true,
                true,
                true,
                CursorPersistenceSnapshot::Invalid,
                CursorAssetSource::SystemTheme,
                DoctorSeverity::Warning,
            ),
            (
                true,
                true,
                true,
                false,
                CursorPersistenceSnapshot::Saved,
                CursorAssetSource::SystemTheme,
                DoctorSeverity::Warning,
            ),
            (
                true,
                false,
                true,
                true,
                CursorPersistenceSnapshot::Saved,
                CursorAssetSource::SystemTheme,
                DoctorSeverity::Ok,
            ),
            (
                true,
                false,
                false,
                true,
                CursorPersistenceSnapshot::Saved,
                CursorAssetSource::SystemTheme,
                DoctorSeverity::Error,
            ),
            (
                false,
                true,
                true,
                true,
                CursorPersistenceSnapshot::Saved,
                CursorAssetSource::SystemTheme,
                DoctorSeverity::Error,
            ),
            (
                true,
                true,
                true,
                true,
                CursorPersistenceSnapshot::Missing,
                CursorAssetSource::BuiltinFallback,
                DoctorSeverity::Warning,
            ),
        ];
        for (
            runtime_available,
            active_theme_available,
            software_fallback_available,
            active_matches_desired,
            persistence,
            asset_source,
            expected,
        ) in cases
        {
            assert_eq!(
                cursor_configuration_doctor_severity(
                    runtime_available,
                    active_theme_available,
                    software_fallback_available,
                    active_matches_desired,
                    persistence,
                    asset_source,
                ),
                expected,
            );
        }
    }
}
