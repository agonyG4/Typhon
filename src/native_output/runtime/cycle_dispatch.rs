use super::cursor_cycle::apply_cursor_position;
use super::*;

use oblivion_one::control::{
    ControlCommand, ControlError, ControlErrorCode, ControlRequest, ControlResponse,
};
use oblivion_one::control_snapshots::{
    ActiveWindowSnapshot, ControlStatusSnapshot, DecorationThemeListSnapshot,
    DecorationThemeSnapshot, DoctorCheck, DoctorSeverity, DoctorSnapshot, FeatureState,
    FeatureStateSnapshot, ModeSnapshot, OutputListSnapshot, OutputSnapshot, PositionSnapshot,
    StatusSnapshot, TrustedEffectsReloadSnapshot, VersionSnapshot, XwaylandStatusSnapshot,
};
use oblivion_one::cursor_manager::{
    CursorIoError, CursorIoOperation, CursorIoSubmitError, CursorJobId, CursorMutationKind,
};
use oblivion_one::native::event_loop::NativeWakeup;
use serde::{Deserialize, Deserializer};

#[inline]
fn input_requires_full_server_progression(
    dispatch_wayland: bool,
    may_change_pointer_constraints: bool,
) -> bool {
    may_change_pointer_constraints && !dispatch_wayland
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativePreReadInputDecision {
    ReadWayland,
    PromoteInputEpoch,
    NoGate,
}

fn decide_native_pre_read_input(
    dispatch_wayland: bool,
    service_input: bool,
    input_ready: bool,
) -> NativePreReadInputDecision {
    if !dispatch_wayland || service_input {
        NativePreReadInputDecision::NoGate
    } else if input_ready {
        NativePreReadInputDecision::PromoteInputEpoch
    } else {
        NativePreReadInputDecision::ReadWayland
    }
}

fn promote_native_input_before_wayland_read<E>(
    dispatch_wayland: bool,
    service_input: &mut bool,
    input_ready: impl FnOnce() -> Result<bool, E>,
) -> Result<NativePreReadInputDecision, E> {
    if !dispatch_wayland || *service_input {
        return Ok(NativePreReadInputDecision::NoGate);
    }

    let decision = decide_native_pre_read_input(dispatch_wayland, *service_input, input_ready()?);
    if matches!(decision, NativePreReadInputDecision::PromoteInputEpoch) {
        *service_input = true;
    }
    Ok(decision)
}

fn timing_transition(transition: NativeInputRoutingTransition) -> NativePointerTimingTransition {
    match transition {
        NativeInputRoutingTransition::LockedActivated(_) => {
            NativePointerTimingTransition::LockedActivated
        }
        NativeInputRoutingTransition::LockedDeactivated(_) => {
            NativePointerTimingTransition::LockedDeactivated
        }
        NativeInputRoutingTransition::ConfinedActivated(_) => {
            NativePointerTimingTransition::ConfinedActivated
        }
        NativeInputRoutingTransition::ConfinedDeactivated(_) => {
            NativePointerTimingTransition::ConfinedDeactivated
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct NativeWaylandInputDispatchOutcome {
    pub(super) pacing_readiness_changed: bool,
    pub(super) routing_transition: Option<NativeInputRoutingTransition>,
    pub(super) vt_switch_requested: Option<u8>,
}

fn settle_native_pointer_constraint_backend_requests(
    input_epoch: &NativeInputEpoch,
    server: &mut OwnCompositorServer,
    backend: &mut NativePointerConstraintBackend,
    input_state: &mut NativeInputState,
    cursor_mode: NativeCursorRenderMode,
    settlement_point: NativeInputConstraintSettlementPoint,
    timing_enabled: bool,
) -> NativeResult<NativePointerConstraintSettlementOutcome> {
    if !input_epoch.constraint_settlement_allowed() {
        let pending = server.pointer_constraint_backend_request_count();
        native_pointer_debug_log_lazy(|| {
            format!(
                "pointer.constraint deferred epoch={:?} reason=input_epoch_active pending={}",
                input_epoch.active_id(),
                pending,
            )
        });
        return Ok(NativePointerConstraintSettlementOutcome::default());
    }
    process_native_pointer_constraint_backend_requests(
        server,
        backend,
        input_state,
        cursor_mode,
        settlement_point,
        timing_enabled,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyCursorArgs {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyKeyboardLayoutArgs {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyboardLayoutSetArgs {
    index: u32,
}

#[derive(Debug)]
struct RequiredNullable<T>(Option<T>);

impl<'de, T> Deserialize<'de> for RequiredNullable<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<T>::deserialize(deserializer).map(Self)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KeyboardConfigurationSetArgs {
    rules: RequiredNullable<String>,
    model: RequiredNullable<String>,
    layout: String,
    variant: RequiredNullable<String>,
    options: RequiredNullable<String>,
    repeat_rate: i32,
    repeat_delay: i32,
    default_layout_index: u32,
}

impl KeyboardConfigurationSetArgs {
    fn into_config(self) -> oblivion_one::compositor::KeyboardConfig {
        oblivion_one::compositor::KeyboardConfig {
            rules: self.rules.0,
            model: self.model.0,
            layout: self.layout,
            variant: self.variant.0,
            options: self.options.0,
            repeat_rate: self.repeat_rate,
            repeat_delay: self.repeat_delay,
            default_layout_index: self.default_layout_index,
        }
    }
}

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

fn dispatch_keyboard_layout_command(
    server: &mut OwnCompositorServer,
    command: ControlCommand,
    request: ControlRequest,
) -> ControlResponse {
    let result = match command {
        ControlCommand::KeyboardLayoutGet => {
            if serde_json::from_value::<EmptyKeyboardLayoutArgs>(request.args).is_err() {
                return keyboard_layout_argument_failure(request.id);
            }
            server.keyboard_layout_snapshot()
        }
        ControlCommand::KeyboardLayoutNext => {
            if serde_json::from_value::<EmptyKeyboardLayoutArgs>(request.args).is_err() {
                return keyboard_layout_argument_failure(request.id);
            }
            server.next_keyboard_layout()
        }
        ControlCommand::KeyboardLayoutPrevious => {
            if serde_json::from_value::<EmptyKeyboardLayoutArgs>(request.args).is_err() {
                return keyboard_layout_argument_failure(request.id);
            }
            server.previous_keyboard_layout()
        }
        ControlCommand::KeyboardLayoutSet => {
            let args = match serde_json::from_value::<KeyboardLayoutSetArgs>(request.args) {
                Ok(args) => args,
                Err(_) => return keyboard_layout_argument_failure(request.id),
            };
            server.set_keyboard_layout(args.index)
        }
        _ => {
            return ControlResponse::failure(
                request.id,
                ControlError::new(
                    ControlErrorCode::InvalidCommand,
                    "command is not a keyboard layout command",
                ),
            );
        }
    };
    match result {
        Ok(snapshot) => match serde_json::to_value(snapshot) {
            Ok(snapshot) => ControlResponse::success(request.id, snapshot),
            Err(_) => ControlResponse::failure(
                request.id,
                ControlError::new(ControlErrorCode::Internal, "control snapshot failed"),
            ),
        },
        Err(error) => keyboard_layout_failure(request.id, error),
    }
}

fn keyboard_configuration_argument_failure(id: u64) -> ControlResponse {
    ControlResponse::failure(
        id,
        ControlError::new(
            ControlErrorCode::InvalidArgument,
            "invalid keyboard configuration arguments",
        )
        .with_detail("invalid_keyboard_configuration_arguments"),
    )
}

fn keyboard_configuration_failure(
    id: u64,
    error: oblivion_one::compositor::KeyboardConfigurationControlError,
) -> ControlResponse {
    match error {
        oblivion_one::compositor::KeyboardConfigurationControlError::InvalidArgument(message) => {
            ControlResponse::failure(
                id,
                ControlError::new(ControlErrorCode::InvalidArgument, message)
                    .with_detail("invalid_keyboard_configuration"),
            )
        }
        oblivion_one::compositor::KeyboardConfigurationControlError::Busy => {
            ControlResponse::failure(
                id,
                ControlError::new(
                    ControlErrorCode::Internal,
                    "keyboard configuration transaction is busy",
                )
                .with_detail("keyboard_configuration_busy"),
            )
        }
        oblivion_one::compositor::KeyboardConfigurationControlError::Unavailable(message) => {
            ControlResponse::failure(
                id,
                ControlError::new(ControlErrorCode::Internal, message)
                    .with_detail("keyboard_state_unavailable"),
            )
        }
        oblivion_one::compositor::KeyboardConfigurationControlError::Internal(message) => {
            ControlResponse::failure(id, ControlError::new(ControlErrorCode::Internal, message))
        }
    }
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

    pub(super) fn service_keyboard_persistence_completions(
        &mut self,
        wakeup: &NativeWakeup,
    ) -> NativeResult<()> {
        let worker_failed_without_readiness = self
            .keyboard_persistence_worker
            .as_ref()
            .is_some_and(|worker| !worker.is_available());
        let worker_ready = wakeup.reasons.keyboard_persistence_worker()
            || !wakeup.keyboard_persistence_events.is_empty()
            || worker_failed_without_readiness;
        if worker_ready {
            let terminal_readiness = wakeup.keyboard_persistence_events.iter().any(|event| {
                event.flags & (libc::EPOLLERR | libc::EPOLLHUP | libc::EPOLLRDHUP) as u32 != 0
            });
            let (notification_error, completion, worker_unavailable) = {
                let Some(worker) = self.keyboard_persistence_worker.as_ref() else {
                    return self.try_commit_keyboard_configuration();
                };
                let notification_error = worker.drain_notification().err();
                let completion = worker.try_completion();
                (notification_error, completion, !worker.is_available())
            };
            let terminal_failure =
                terminal_readiness || worker_unavailable || notification_error.is_some();
            if let Some(completion) = completion {
                if self
                    .pending_keyboard_job
                    .as_ref()
                    .is_none_or(|pending| pending.job_id != completion.job_id)
                {
                    // A completion from an abandoned job cannot affect the active transaction.
                    if terminal_failure {
                        self.disable_keyboard_persistence_worker();
                    }
                    return self.try_commit_keyboard_configuration();
                }
                match completion.result {
                    Ok(()) => {
                        if self.server.mark_keyboard_configuration_persisted().is_err() {
                            self.fail_keyboard_configuration(
                                ControlErrorCode::Internal,
                                "keyboard configuration persistence acknowledgement failed",
                                "keyboard_configuration_internal",
                            )?;
                        }
                    }
                    Err(error) => {
                        self.server.abort_keyboard_configuration();
                        let pending = self
                            .pending_keyboard_job
                            .take()
                            .expect("keyboard completion had a pending job");
                        let mut response = ControlResponse::failure(
                            pending.request_id,
                            ControlError::new(ControlErrorCode::Internal, error.to_string()),
                        );
                        if let Some(response_error) = response.error.as_mut() {
                            response_error.detail = Some("keyboard_persistence_failed".to_string());
                        }
                        self.queue_keyboard_response_if_connected(pending.token, response)?;
                    }
                }
                self.try_commit_keyboard_configuration()?;
            } else if terminal_failure && self.pending_keyboard_job.is_some() {
                self.server.abort_keyboard_configuration();
                let pending = self
                    .pending_keyboard_job
                    .take()
                    .expect("keyboard pending job checked above");
                let response = ControlResponse::failure(
                    pending.request_id,
                    ControlError::new(
                        ControlErrorCode::Internal,
                        "keyboard configuration persistence unavailable",
                    )
                    .with_detail("keyboard_persistence_unavailable"),
                );
                self.queue_keyboard_response_if_connected(pending.token, response)?;
            }
            if terminal_failure {
                self.disable_keyboard_persistence_worker();
            }
        }
        self.try_commit_keyboard_configuration()
    }

    fn try_commit_keyboard_configuration(&mut self) -> NativeResult<()> {
        let should_commit = self.pending_keyboard_job.is_some()
            && self.server.keyboard_configuration_pending_is_persisted()
            && self.server.keyboard_reconfiguration_is_quiescent();
        if !should_commit {
            return Ok(());
        }
        let pending = self
            .pending_keyboard_job
            .take()
            .expect("keyboard commit readiness checked above");
        let response = match self.server.commit_keyboard_configuration() {
            Ok(mutation) => match serde_json::to_value(mutation.snapshot) {
                Ok(snapshot) => ControlResponse::success(pending.request_id, snapshot),
                Err(_) => ControlResponse::failure(
                    pending.request_id,
                    ControlError::new(
                        ControlErrorCode::Internal,
                        "keyboard configuration snapshot serialization failed",
                    ),
                ),
            },
            Err(error) => {
                self.server.abort_keyboard_configuration();
                keyboard_configuration_failure(pending.request_id, error)
            }
        };
        self.queue_keyboard_response_if_connected(pending.token, response)
    }

    fn queue_keyboard_response_if_connected(
        &mut self,
        token: oblivion_one::native::event_loop::ReactorToken,
        response: ControlResponse,
    ) -> NativeResult<()> {
        if self.control_server.has_client(token) {
            self.control_server
                .queue_response(&mut self.event_loop, token, response)?;
        }
        Ok(())
    }

    fn fail_keyboard_configuration(
        &mut self,
        code: ControlErrorCode,
        message: &'static str,
        detail: &'static str,
    ) -> NativeResult<()> {
        self.server.abort_keyboard_configuration();
        let Some(pending) = self.pending_keyboard_job.take() else {
            return Ok(());
        };
        let mut response =
            ControlResponse::failure(pending.request_id, ControlError::new(code, message));
        if let Some(error) = response.error.as_mut() {
            error.detail = Some(detail.to_string());
        }
        self.queue_keyboard_response_if_connected(pending.token, response)
    }

    fn disable_keyboard_persistence_worker(&mut self) {
        if let Some(token) = self.keyboard_persistence_worker_reactor_token.take() {
            let _ = self.event_loop.unregister(token);
        }
        self.keyboard_persistence_worker.take();
    }

    fn keyboard_configuration_snapshot_response(&mut self, request_id: u64) -> ControlResponse {
        match self.server.keyboard_configuration_snapshot() {
            Ok(snapshot) => match serde_json::to_value(snapshot) {
                Ok(snapshot) => ControlResponse::success(request_id, snapshot),
                Err(_) => ControlResponse::failure(
                    request_id,
                    ControlError::new(
                        ControlErrorCode::Internal,
                        "keyboard configuration snapshot serialization failed",
                    ),
                ),
            },
            Err(error) => keyboard_configuration_failure(request_id, error),
        }
    }

    fn queue_keyboard_configuration(
        &mut self,
        token: oblivion_one::native::event_loop::ReactorToken,
        request_id: u64,
        configuration: oblivion_one::compositor::KeyboardConfig,
    ) -> Option<ControlResponse> {
        if self.pending_keyboard_job.is_some() || self.server.keyboard_configuration_pending() {
            return Some(keyboard_configuration_failure(
                request_id,
                oblivion_one::compositor::KeyboardConfigurationControlError::Busy,
            ));
        }
        let preparation = match self
            .server
            .prepare_keyboard_configuration(configuration.clone())
        {
            Ok(preparation) => preparation,
            Err(error) => return Some(keyboard_configuration_failure(request_id, error)),
        };
        if matches!(
            preparation,
            oblivion_one::compositor::KeyboardConfigurationPreparation::NoOp
        ) {
            return Some(self.keyboard_configuration_snapshot_response(request_id));
        }
        let Some(worker) = self.keyboard_persistence_worker.as_ref() else {
            self.server.abort_keyboard_configuration();
            return Some(ControlResponse::failure(
                request_id,
                ControlError::new(
                    ControlErrorCode::Internal,
                    "keyboard configuration persistence unavailable",
                )
                .with_detail("keyboard_persistence_unavailable"),
            ));
        };
        let job_id = oblivion_one::keyboard_persistence::KeyboardJobId(self.next_keyboard_job_id);
        self.next_keyboard_job_id = self.next_keyboard_job_id.saturating_add(1);
        if let Err(error) = worker.submit(
            oblivion_one::keyboard_persistence::KeyboardPersistenceOperation::Write {
                job_id,
                configuration,
            },
        ) {
            self.server.abort_keyboard_configuration();
            let (message, detail) = match error {
                oblivion_one::keyboard_persistence::KeyboardPersistenceSubmitError::Busy => (
                    "keyboard configuration persistence is busy",
                    "keyboard_persistence_busy",
                ),
                oblivion_one::keyboard_persistence::KeyboardPersistenceSubmitError::Unavailable => {
                    (
                        "keyboard configuration persistence unavailable",
                        "keyboard_persistence_unavailable",
                    )
                }
            };
            let mut response = ControlResponse::failure(
                request_id,
                ControlError::new(ControlErrorCode::Internal, message),
            );
            if let Some(error) = response.error.as_mut() {
                error.detail = Some(detail.to_string());
            }
            return Some(response);
        }
        self.pending_keyboard_job = Some(PendingKeyboardJob {
            token,
            request_id,
            job_id,
        });
        None
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
        if matches!(
            command,
            ControlCommand::KeyboardLayoutGet
                | ControlCommand::KeyboardLayoutNext
                | ControlCommand::KeyboardLayoutPrevious
                | ControlCommand::KeyboardLayoutSet
        ) {
            return Some(dispatch_keyboard_layout_command(
                &mut self.server,
                command,
                request,
            ));
        }
        if command == ControlCommand::KeyboardConfigurationGet {
            if serde_json::from_value::<EmptyKeyboardLayoutArgs>(request.args).is_err() {
                return Some(keyboard_configuration_argument_failure(request.id));
            }
            return Some(self.keyboard_configuration_snapshot_response(request.id));
        }
        if command == ControlCommand::KeyboardConfigurationSet {
            let args = match serde_json::from_value::<KeyboardConfigurationSetArgs>(request.args) {
                Ok(args) => args,
                Err(_) => return Some(keyboard_configuration_argument_failure(request.id)),
            };
            return self.queue_keyboard_configuration(token, request.id, args.into_config());
        }
        if command == ControlCommand::EffectsReload {
            if serde_json::from_value::<EmptyCursorArgs>(request.args).is_err() {
                return Some(ControlResponse::failure(
                    request.id,
                    ControlError::new(
                        ControlErrorCode::InvalidArgument,
                        "effects reload takes no arguments",
                    ),
                ));
            }
            let snapshot = match super::reload_trusted_effects_from_disk(
                &mut self.scanout,
                &mut self.server,
            ) {
                Ok(Some(snapshot)) => {
                    super::request_trusted_effect_reload_redraw(&mut self.queued_redraw_requested);
                    snapshot
                }
                Ok(None) => {
                    return Some(ControlResponse::failure(
                        request.id,
                        ControlError::new(
                            ControlErrorCode::Internal,
                            "trusted effects manifest is not present",
                        ),
                    ));
                }
                Err(error) => {
                    return Some(ControlResponse::failure(
                        request.id,
                        ControlError::new(ControlErrorCode::Internal, error),
                    ));
                }
            };
            return Some(
                match serde_json::to_value::<TrustedEffectsReloadSnapshot>(snapshot) {
                    Ok(result) => ControlResponse::success(request.id, result),
                    Err(_) => ControlResponse::failure(
                        request.id,
                        ControlError::new(
                            ControlErrorCode::Internal,
                            "effect reload snapshot failed",
                        ),
                    ),
                },
            );
        }
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
        service_input: bool,
        dispatch_wayland: bool,
    ) -> NativeResult<NativeWaylandInputDispatchOutcome> {
        let mut service_input = service_input;
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
            input_epoch,
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
            process_supervisor,
            render_telemetry,
            pointer_timing,
            #[cfg(test)]
            native_io_recorder,
            shutdown: _,
            session: _,
            ..
        } = self;
        let present_us = 0;
        let pageflip_pending_at_tick = scanout.page_flip_pending();
        let mut accepted = 0;
        let mut tick_us = 0;
        let mut pacing_readiness_changed = false;
        let mut vt_switch_requested = None;
        let mut input_drain_us = 0;
        let mut raw_input_events = 0;
        let mut coalesced_input_events = 0;
        let mut input_event_timestamp_usec = None;
        let timing_enabled = pointer_timing.enabled();
        let pending_constraint_requests_before_settlement =
            server.pointer_constraint_backend_request_count();
        native_pointer_debug_log_lazy(|| {
            format!(
                "pointer.constraint pending_before_settlement={} epoch_active={:?} backlog_pending={}",
                pending_constraint_requests_before_settlement,
                input_epoch.active_id(),
                input_epoch.backlog_pending(),
            )
        });
        let initial_settlement_timing_enabled = timing_enabled
            && input_epoch.constraint_settlement_allowed()
            && pending_constraint_requests_before_settlement > 0;
        let initial_settlement_start = initial_settlement_timing_enabled
            .then(capture_timing_point)
            .transpose()?;
        let initial_settlement = settle_native_pointer_constraint_backend_requests(
            input_epoch,
            server,
            pointer_constraint_backend,
            input_state,
            *cursor_render_mode,
            NativeInputConstraintSettlementPoint::BeforeInputEpoch,
            initial_settlement_timing_enabled,
        )?;
        let initial_settlement_end = initial_settlement_timing_enabled
            .then(capture_timing_point)
            .transpose()?;
        let mut redraw_requested = initial_settlement.redraw_requested;
        let initial_transition_evidence = initial_settlement.selected_transition;
        let mut routing_transition =
            initial_transition_evidence.map(|evidence| evidence.transition);
        if timing_enabled && let Some(evidence) = initial_transition_evidence {
            pointer_timing.record_routing_transition_committed_with_pre_read_timed(
                timing_transition(evidence.transition),
                capture_timing_point()?,
                NativePointerPreReadObservation {
                    constraint_settlement_start: initial_settlement_start,
                    constraint_settlement_end: initial_settlement_end,
                    constraint_activation_start: evidence.action_timing.activation_start,
                    constraint_activation_end: evidence.action_timing.activation_end,
                    wayland_flush_start: evidence.action_timing.wayland_flush_start,
                    wayland_flush_end: evidence.action_timing.wayland_flush_end,
                    constraint_region_resolution_duration_ns: evidence
                        .constraint_region_resolution_duration_ns,
                    constraint_region_resolution_thread_cpu_ns: evidence
                        .constraint_region_resolution_thread_cpu_ns,
                    transition_context: evidence.transition_context,
                    ..Default::default()
                },
            );
        }
        let cursor_sync_start_at_ns = timing_enabled.then(monotonic_now_ns).transpose()?;
        synchronize_cursor_state_for_server(server, atomic_cursor, legacy_cursor, input_state)?;
        if let Some(start_ns) = cursor_sync_start_at_ns {
            pointer_timing.record_cursor_sync(start_ns, monotonic_now_ns()?);
        }
        let pre_read_probe_performed = dispatch_wayland && !service_input;
        let pre_read_probe_start = if pre_read_probe_performed && timing_enabled {
            Some(capture_timing_point()?)
        } else {
            None
        };
        // A Wayland-only wake owns a final semantic arbitration cut here.
        // Query the backend's exact input registrations rather than the
        // bounded global reactor snapshot, which may omit a ready input
        // source under unrelated-fd saturation.
        let pre_read_input_promoted = matches!(
            promote_native_input_before_wayland_read(dispatch_wayland, &mut service_input, || {
                input_devices.ready_nonblocking()
            },)?,
            NativePreReadInputDecision::PromoteInputEpoch
        );
        let pre_read_probe_end = if pre_read_probe_performed && timing_enabled {
            Some(capture_timing_point()?)
        } else {
            None
        };
        let mut pre_read_observation = NativePointerPreReadObservation {
            probe_performed: pre_read_probe_performed,
            input_promoted: pre_read_input_promoted,
            pre_read_probe_start,
            pre_read_probe_end,
            batch: None,
            ..Default::default()
        };
        // A serviceable native input queue owns the semantic epoch.  Read-side
        // Wayland work is intentionally performed after that epoch so requests
        // that create or alter input resources cannot reinterpret its events.
        let pending_constraint_requests_before_read =
            server.pointer_constraint_backend_request_count();
        if dispatch_wayland && !service_input {
            let wayland_read_start = timing_enabled.then(capture_timing_point).transpose()?;
            let tick_start = Instant::now();
            let (dispatch_accepted, dispatch_pacing_readiness_changed) =
                server.dispatch_wayland_with_outcome()?;
            if wayland_read_start.is_some() {
                let wayland_read_end = capture_timing_point()?;
                pre_read_observation.wayland_read_start = wayland_read_start;
                pre_read_observation.wayland_read_end = Some(wayland_read_end);
            }
            render_telemetry.resource_efficiency.record_client_flush();
            accepted = dispatch_accepted;
            tick_us = elapsed_micros(tick_start);
            pacing_readiness_changed = dispatch_pacing_readiness_changed;
        }
        let mut pending_constraint_requests_after_read =
            server.pointer_constraint_backend_request_count();
        if dispatch_wayland && !service_input {
            native_pointer_debug_log_lazy(|| {
                format!(
                    "wayland.input_read dispatch after_epoch=false pending={} queued_during_read={}",
                    pending_constraint_requests_after_read,
                    pending_constraint_requests_after_read
                        .saturating_sub(pending_constraint_requests_before_read),
                )
            });
        }
        let mut skipped_input_repaints = 0usize;
        let mut completed_input_epoch = None;
        if service_input {
            #[cfg(test)]
            native_io_recorder.record(NativeIoOperation::RawInputAction);
            if timing_enabled {
                pointer_timing.record_input_service_start(monotonic_now_ns()?);
            }
            server.begin_native_input_batch();
            let continuation = input_epoch.backlog_pending();
            let input_epoch_id = input_epoch.begin(continuation);
            let constraint = pointer_constraint_backend
                .active
                .as_ref()
                .map(|constraint| (constraint.id, constraint.mode));
            input_state
                .set_native_input_epoch_debug(Some(input_epoch_id), constraint.map(|(id, _)| id));
            native_pointer_debug_log_lazy(|| {
                format!(
                    "input.semantic_epoch begin id={} continuation={} constraint={:?} backlog_pending={} wayland_read_deferred={}",
                    input_epoch_id,
                    continuation,
                    constraint,
                    input_epoch.backlog_pending(),
                    true,
                )
            });
            let input_drain_start = Instant::now();
            let libinput_dispatch_start_at_ns = (timing_enabled
                && !continuation
                && matches!(
                    input_devices.kind(),
                    NativeInputBackendKind::LibseatLibinputUdev
                        | NativeInputBackendKind::DirectLibinputUdev
                ))
            .then(monotonic_now_ns)
            .transpose()?;
            let dispatch_succeeded = continuation || input_devices.begin_semantic_epoch();
            if let Some(start_ns) = libinput_dispatch_start_at_ns {
                pointer_timing.record_libinput_dispatch(start_ns, monotonic_now_ns()?);
            }
            let queue_drain_start_at_ns = timing_enabled.then(monotonic_now_ns).transpose()?;
            let queue_drain_start = Instant::now();
            if dispatch_succeeded {
                input_devices.drain_epoch_chunk_into(input_batch);
            } else {
                input_batch.raw.clear();
                input_batch.coalesced.clear();
                input_batch.budget_exhausted = false;
            }
            let queue_drain_end_at_ns = timing_enabled.then(monotonic_now_ns).transpose()?;
            if let Some(start_ns) = queue_drain_start_at_ns {
                pointer_timing.record_queue_drain(
                    start_ns,
                    queue_drain_end_at_ns.expect("timing end recorded with timing start"),
                );
                pointer_timing.record_native_batch_materialized(
                    queue_drain_end_at_ns.expect("timing end recorded with timing start"),
                );
            }
            input_drain_us = elapsed_micros(input_drain_start);
            raw_input_events = input_batch.raw.len();
            // The backend state captured here is authoritative for every event in
            // this epoch. Protocol progress below may queue a new transition, but
            // it cannot change the native semantics of this materialized batch.
            for _ in 0..raw_input_events {
                render_telemetry
                    .resource_efficiency
                    .record_raw_input_event();
            }
            input_event_timestamp_usec = matches!(
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
            let oldest_input_timestamp_usec = input_batch
                .raw
                .iter()
                .filter_map(|event| event.timestamp_usec())
                .min();
            let newest_input_timestamp_usec = input_batch
                .raw
                .iter()
                .filter_map(|event| event.timestamp_usec())
                .max();
            input_batch.coalesce_pointer_motion_events();
            coalesced_input_events = input_batch.coalesced.len();
            let timing_batch = NativePointerTimingBatch {
                raw_events: raw_input_events as u32,
                coalesced_events: coalesced_input_events as u32,
                oldest_hardware_timestamp_us: oldest_input_timestamp_usec,
                newest_hardware_timestamp_us: newest_input_timestamp_usec,
            };
            if timing_enabled {
                pointer_timing.observe_first_batch(timing_batch, monotonic_now_ns()?);
                if pre_read_input_promoted {
                    pre_read_observation.batch = Some(timing_batch);
                }
            }
            native_pointer_debug_log_lazy(|| {
                format!(
                    "input.semantic_epoch batch id={:?} raw={} coalesced={} oldest_ts_us={:?} newest_ts_us={:?} budget_exhausted={} continuation={}",
                    input_epoch_id,
                    raw_input_events,
                    coalesced_input_events,
                    oldest_input_timestamp_usec,
                    newest_input_timestamp_usec,
                    input_batch.budget_exhausted,
                    input_batch.budget_exhausted,
                )
            });
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
                        process_supervisor,
                        xwayland: xwayland_app_environment,
                    },
                ) {
                    Ok(application) => application,
                    Err(error) => {
                        let _ = server.end_native_input_batch();
                        return Err(error);
                    }
                };
                vt_switch_requested = vt_switch_requested.or(application.vt_switch_requested);
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
                    TriggerLivenessPoint::Event(event_index),
                );
                redraw_requested |= interaction_reconciled;
                // A semantic input epoch cannot perform a client read-side
                // dispatch. Remember the narrow native-only progression request
                // and service it once after the epoch has ended.
                if input_requires_full_server_progression(
                    dispatch_wayland,
                    may_change_pointer_constraints,
                ) {
                    input_epoch.request_deferred_wayland_progression();
                    native_pointer_debug_log_lazy(|| {
                        format!(
                            "wayland.input_read deferred epoch={:?} reason=protocol_progression",
                            input_epoch.active_id(),
                        )
                    });
                }
            }
            let interaction_reconciled =
                reconcile_trigger_liveness(server, input_state, TriggerLivenessPoint::BatchEnd);
            redraw_requested |= interaction_reconciled;
            completed_input_epoch = Some(input_epoch_id);
            input_epoch.finish(input_batch.budget_exhausted);
            native_pointer_debug_log_lazy(|| {
                format!(
                    "input.semantic_epoch end id={:?} raw={} coalesced={} budget_exhausted={} backlog_pending={}",
                    completed_input_epoch,
                    raw_input_events,
                    coalesced_input_events,
                    input_batch.budget_exhausted,
                    input_epoch.backlog_pending(),
                )
            });
            if !input_epoch.backlog_pending() {
                input_state.set_native_input_epoch_debug(None, None);
            }
            let cursor_sync_start_at_ns = timing_enabled.then(monotonic_now_ns).transpose()?;
            if let Err(error) = synchronize_cursor_state_for_server(
                server,
                atomic_cursor,
                legacy_cursor,
                input_state,
            ) {
                let _ = server.end_native_input_batch();
                return Err(error.into());
            }
            if let Some(start_ns) = cursor_sync_start_at_ns {
                pointer_timing.record_cursor_sync(start_ns, monotonic_now_ns()?);
            }
            let _ = observe_atomic_cursor_output_liveness(
                atomic_cursor.as_ref(),
                cursor_output_arbitration,
                frame_scheduler,
                monotonic_now_ns()?,
                *cursor_render_mode,
                input_state.cursor_visible(),
            );
            let client_flush = server.end_native_input_batch()?;
            if timing_enabled {
                pointer_timing.record_input_service_end(monotonic_now_ns()?);
            }
            if client_flush {
                render_telemetry.resource_efficiency.record_client_flush();
            }
        }
        let deferred_wayland_progression = if !input_epoch.backlog_pending() {
            input_epoch.take_deferred_wayland_progression()
        } else {
            false
        };
        let input_backlog_continuation = service_input && input_epoch.backlog_pending();
        let should_dispatch_after_input = service_input
            && !input_backlog_continuation
            && (dispatch_wayland || deferred_wayland_progression);
        if should_dispatch_after_input {
            let wayland_read_start = timing_enabled.then(capture_timing_point).transpose()?;
            let tick_start = Instant::now();
            let pending_before_read = server.pointer_constraint_backend_request_count();
            let (dispatch_accepted, dispatch_pacing_readiness_changed) =
                server.dispatch_wayland_with_outcome()?;
            if wayland_read_start.is_some() {
                let wayland_read_end = capture_timing_point()?;
                pre_read_observation.wayland_read_start = wayland_read_start;
                pre_read_observation.wayland_read_end = Some(wayland_read_end);
            }
            accepted = dispatch_accepted;
            tick_us = elapsed_micros(tick_start);
            pacing_readiness_changed = dispatch_pacing_readiness_changed;
            render_telemetry.resource_efficiency.record_client_flush();
            pending_constraint_requests_after_read =
                server.pointer_constraint_backend_request_count();
            native_pointer_debug_log_lazy(|| {
                format!(
                    "wayland.input_read dispatch after_epoch=true pending={} queued_during_read={}",
                    pending_constraint_requests_after_read,
                    pending_constraint_requests_after_read.saturating_sub(pending_before_read),
                )
            });
            if deferred_wayland_progression {
                redraw_requested |= server.progress_surface_pacing(monotonic_now_ns()?)?;
            }
        }
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
        let final_settlement_timing_enabled = timing_enabled
            && input_epoch.constraint_settlement_allowed()
            && server.pointer_constraint_backend_request_count() > 0;
        let final_settlement_start = final_settlement_timing_enabled
            .then(capture_timing_point)
            .transpose()?;
        let final_settlement = settle_native_pointer_constraint_backend_requests(
            input_epoch,
            server,
            pointer_constraint_backend,
            input_state,
            *cursor_render_mode,
            if let Some(epoch) = completed_input_epoch {
                NativeInputConstraintSettlementPoint::AfterInputEpoch(Some(epoch))
            } else {
                NativeInputConstraintSettlementPoint::BeforeInputEpoch
            },
            final_settlement_timing_enabled,
        )?;
        let final_settlement_end = final_settlement_timing_enabled
            .then(capture_timing_point)
            .transpose()?;
        pre_read_observation.constraint_settlement_start = final_settlement_start;
        pre_read_observation.constraint_settlement_end = final_settlement_end;
        let final_transition_evidence = final_settlement.selected_transition;
        if let Some(evidence) = final_transition_evidence {
            pre_read_observation.constraint_activation_start =
                evidence.action_timing.activation_start;
            pre_read_observation.constraint_activation_end = evidence.action_timing.activation_end;
            pre_read_observation.wayland_flush_start = evidence.action_timing.wayland_flush_start;
            pre_read_observation.wayland_flush_end = evidence.action_timing.wayland_flush_end;
            pre_read_observation.constraint_region_resolution_duration_ns =
                evidence.constraint_region_resolution_duration_ns;
            pre_read_observation.constraint_region_resolution_thread_cpu_ns =
                evidence.constraint_region_resolution_thread_cpu_ns;
            pre_read_observation.transition_context = evidence.transition_context;
        } else {
            pre_read_observation.constraint_activation_start = None;
            pre_read_observation.constraint_activation_end = None;
            pre_read_observation.wayland_flush_start = None;
            pre_read_observation.wayland_flush_end = None;
            pre_read_observation.constraint_region_resolution_duration_ns = None;
            pre_read_observation.constraint_region_resolution_thread_cpu_ns = None;
            pre_read_observation.transition_context = None;
        }
        redraw_requested |= final_settlement.redraw_requested;
        if routing_transition.is_none() {
            routing_transition = final_transition_evidence.map(|evidence| evidence.transition);
        }
        if timing_enabled && let Some(evidence) = final_transition_evidence {
            let committed_at = capture_timing_point()?;
            pointer_timing.record_routing_transition_committed_with_pre_read_timed(
                timing_transition(evidence.transition),
                committed_at,
                pre_read_observation,
            );
        }
        let cursor_sync_start_at_ns = timing_enabled.then(monotonic_now_ns).transpose()?;
        if let Err(error) =
            synchronize_cursor_state_for_server(server, atomic_cursor, legacy_cursor, input_state)
        {
            return Err(error.into());
        }
        if let Some(start_ns) = cursor_sync_start_at_ns {
            pointer_timing.record_cursor_sync(start_ns, monotonic_now_ns()?);
        }
        if timing_enabled {
            pointer_timing.record_dispatch_return(monotonic_now_ns()?);
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
        Ok(NativeWaylandInputDispatchOutcome {
            pacing_readiness_changed,
            routing_transition,
            vt_switch_requested,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EmptyKeyboardLayoutArgs, KeyboardConfigurationSetArgs, KeyboardLayoutSetArgs,
        NativePreReadInputDecision, decide_native_pre_read_input, dispatch_keyboard_layout_command,
        input_requires_full_server_progression, keyboard_layout_failure,
        promote_native_input_before_wayland_read,
    };
    use crate::native_output::input::NativeInputEpoch;
    use oblivion_one::{
        compositor::{KeyboardLayoutControlError, OwnCompositorServer},
        control::{ControlCommand, ControlRequest},
    };
    use std::sync::Mutex;

    static KEYBOARD_LAYOUT_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ConstraintMode {
        None,
        Locked(u64),
        Confined(u64),
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ConstraintTransition {
        ActivateLocked(u64),
        ActivateConfined(u64),
        Deactivate,
    }

    #[test]
    fn keyboard_layout_control_arguments_are_strict() {
        assert!(serde_json::from_value::<EmptyKeyboardLayoutArgs>(serde_json::json!({})).is_ok());
        assert!(
            serde_json::from_value::<EmptyKeyboardLayoutArgs>(serde_json::json!({"extra": true}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<KeyboardLayoutSetArgs>(serde_json::json!({"index": 1}))
                .is_ok()
        );
        for value in [
            serde_json::json!({}),
            serde_json::json!({"index": 1, "extra": true}),
            serde_json::json!({"index": -1}),
        ] {
            assert!(serde_json::from_value::<KeyboardLayoutSetArgs>(value).is_err());
        }
    }

    #[test]
    fn keyboard_layout_errors_keep_protocol_categories() {
        let invalid = keyboard_layout_failure(
            7,
            KeyboardLayoutControlError::InvalidIndex { index: 3, count: 2 },
        );
        assert_eq!(invalid.error.unwrap().code.as_str(), "invalid_argument");
        let unavailable = keyboard_layout_failure(
            8,
            KeyboardLayoutControlError::Unavailable("keyboard state unavailable"),
        );
        let error = unavailable.error.unwrap();
        assert_eq!(error.code.as_str(), "internal");
        assert_eq!(error.detail.as_deref(), Some("keyboard_state_unavailable"));
    }

    #[test]
    fn keyboard_configuration_set_requires_the_complete_typed_document() {
        let valid = serde_json::json!({
            "rules": null,
            "model": null,
            "layout": "us",
            "variant": null,
            "options": null,
            "repeatRate": 25,
            "repeatDelay": 600,
            "defaultLayoutIndex": 0
        });
        assert!(serde_json::from_value::<KeyboardConfigurationSetArgs>(valid.clone()).is_ok());
        for value in [
            serde_json::json!({"layout": "us"}),
            {
                let mut value = valid.clone();
                value["extra"] = serde_json::json!(true);
                value
            },
            {
                let mut value = valid;
                value["repeatRate"] = serde_json::json!("25");
                value
            },
        ] {
            assert!(serde_json::from_value::<KeyboardConfigurationSetArgs>(value).is_err());
        }
    }

    #[test]
    fn keyboard_layout_dispatch_qualifies_the_live_server_sequence() {
        let _guard = KEYBOARD_LAYOUT_ENV_LOCK.lock().unwrap();
        let previous_layout = std::env::var_os("OBLIVION_ONE_XKB_LAYOUT");
        let previous_variant = std::env::var_os("OBLIVION_ONE_XKB_VARIANT");
        let previous_options = std::env::var_os("OBLIVION_ONE_XKB_OPTIONS");
        // SAFETY: this test serializes its process-wide environment changes.
        unsafe {
            std::env::set_var("OBLIVION_ONE_XKB_LAYOUT", "br,us");
            std::env::set_var("OBLIVION_ONE_XKB_VARIANT", "abnt2,");
            std::env::set_var("OBLIVION_ONE_XKB_OPTIONS", "");
        }

        let socket_name = format!(
            "typhon-keyboard-control-dispatch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
        let dispatch = |server: &mut OwnCompositorServer, command, args| {
            dispatch_keyboard_layout_command(
                server,
                command,
                ControlRequest::new(1, command.as_str(), args).unwrap(),
            )
        };
        let snapshot = |response: oblivion_one::control::ControlResponse| {
            assert!(
                response.ok,
                "unexpected control error: {:?}",
                response.error
            );
            serde_json::from_value(response.result.unwrap()).unwrap()
        };

        let initial: oblivion_one::control_snapshots::KeyboardLayoutSnapshot = snapshot(dispatch(
            &mut server,
            ControlCommand::KeyboardLayoutGet,
            serde_json::json!({}),
        ));
        assert_eq!((initial.effective_index, initial.locked_index), (0, 0));
        assert_eq!(initial.layout_count, 2);

        let selected: oblivion_one::control_snapshots::KeyboardLayoutSnapshot = snapshot(dispatch(
            &mut server,
            ControlCommand::KeyboardLayoutSet,
            serde_json::json!({"index": 1}),
        ));
        assert_eq!((selected.effective_index, selected.locked_index), (1, 1));

        let queried: oblivion_one::control_snapshots::KeyboardLayoutSnapshot = snapshot(dispatch(
            &mut server,
            ControlCommand::KeyboardLayoutGet,
            serde_json::json!({}),
        ));
        assert_eq!(queried.locked_index, 1);

        let next: oblivion_one::control_snapshots::KeyboardLayoutSnapshot = snapshot(dispatch(
            &mut server,
            ControlCommand::KeyboardLayoutNext,
            serde_json::json!({}),
        ));
        assert_eq!(next.locked_index, 0);

        let previous: oblivion_one::control_snapshots::KeyboardLayoutSnapshot = snapshot(dispatch(
            &mut server,
            ControlCommand::KeyboardLayoutPrevious,
            serde_json::json!({}),
        ));
        assert_eq!(previous.locked_index, 1);

        let no_op: oblivion_one::control_snapshots::KeyboardLayoutSnapshot = snapshot(dispatch(
            &mut server,
            ControlCommand::KeyboardLayoutSet,
            serde_json::json!({"index": 1}),
        ));
        assert_eq!(no_op, previous);

        let invalid = dispatch(
            &mut server,
            ControlCommand::KeyboardLayoutSet,
            serde_json::json!({"index": 2}),
        );
        assert_eq!(invalid.error.unwrap().code.as_str(), "invalid_argument");
        for (command, args) in [
            (
                ControlCommand::KeyboardLayoutGet,
                serde_json::json!({"extra": true}),
            ),
            (
                ControlCommand::KeyboardLayoutNext,
                serde_json::json!({"extra": true}),
            ),
            (
                ControlCommand::KeyboardLayoutPrevious,
                serde_json::json!({"extra": true}),
            ),
            (ControlCommand::KeyboardLayoutSet, serde_json::json!({})),
            (
                ControlCommand::KeyboardLayoutSet,
                serde_json::json!({"index": -1}),
            ),
            (
                ControlCommand::KeyboardLayoutSet,
                serde_json::json!({"index": 1.5}),
            ),
            (
                ControlCommand::KeyboardLayoutSet,
                serde_json::json!({"index": "1"}),
            ),
            (
                ControlCommand::KeyboardLayoutSet,
                serde_json::json!({"index": 1, "extra": true}),
            ),
        ] {
            let response = dispatch(&mut server, command, args);
            assert_eq!(response.error.unwrap().code.as_str(), "invalid_argument");
        }

        drop(server);
        unsafe {
            std::env::set_var("OBLIVION_ONE_XKB_LAYOUT", "br");
            std::env::set_var("OBLIVION_ONE_XKB_VARIANT", "abnt2");
        }
        let mut single_layout_server = OwnCompositorServer::bind(&socket_name).unwrap();
        for command in [
            ControlCommand::KeyboardLayoutNext,
            ControlCommand::KeyboardLayoutPrevious,
            ControlCommand::KeyboardLayoutSet,
        ] {
            let args = if command == ControlCommand::KeyboardLayoutSet {
                serde_json::json!({"index": 0})
            } else {
                serde_json::json!({})
            };
            let response = dispatch(&mut single_layout_server, command, args);
            let snapshot: oblivion_one::control_snapshots::KeyboardLayoutSnapshot =
                snapshot(response);
            assert_eq!(snapshot.locked_index, 0);
            assert_eq!(snapshot.layout_count, 1);
        }
        drop(single_layout_server);
        unsafe {
            match previous_layout {
                Some(value) => std::env::set_var("OBLIVION_ONE_XKB_LAYOUT", value),
                None => std::env::remove_var("OBLIVION_ONE_XKB_LAYOUT"),
            }
            match previous_variant {
                Some(value) => std::env::set_var("OBLIVION_ONE_XKB_VARIANT", value),
                None => std::env::remove_var("OBLIVION_ONE_XKB_VARIANT"),
            }
            match previous_options {
                Some(value) => std::env::set_var("OBLIVION_ONE_XKB_OPTIONS", value),
                None => std::env::remove_var("OBLIVION_ONE_XKB_OPTIONS"),
            }
        }
    }

    fn settle_if_allowed(
        epoch: &NativeInputEpoch,
        mode: &mut ConstraintMode,
        transition: ConstraintTransition,
    ) {
        if !epoch.constraint_settlement_allowed() {
            return;
        }
        *mode = match transition {
            ConstraintTransition::ActivateLocked(generation) => ConstraintMode::Locked(generation),
            ConstraintTransition::ActivateConfined(generation) => {
                ConstraintMode::Confined(generation)
            }
            ConstraintTransition::Deactivate => ConstraintMode::None,
        };
    }

    fn record_motion(
        epoch: &NativeInputEpoch,
        mode: ConstraintMode,
        delta: f64,
        result: &mut Vec<(Option<u64>, ConstraintMode, f64)>,
    ) {
        result.push((epoch.active_id(), mode, delta));
    }

    #[test]
    fn current_dispatch_activation_waits_until_the_current_input_epoch_ends() {
        let mut epoch = NativeInputEpoch::default();
        let mut mode = ConstraintMode::None;
        let mut results = Vec::new();
        epoch.begin(false);

        settle_if_allowed(&epoch, &mut mode, ConstraintTransition::ActivateLocked(7));
        record_motion(&epoch, mode, 60.0, &mut results);
        epoch.finish(false);
        settle_if_allowed(&epoch, &mut mode, ConstraintTransition::ActivateLocked(7));
        epoch.begin(false);
        record_motion(&epoch, mode, 5.0, &mut results);

        assert_eq!(
            results,
            vec![
                (Some(1), ConstraintMode::None, 60.0),
                (Some(2), ConstraintMode::Locked(7), 5.0),
            ]
        );
    }

    #[test]
    fn pre_existing_activation_is_effective_before_the_new_input_epoch() {
        let mut epoch = NativeInputEpoch::default();
        let mut mode = ConstraintMode::None;
        let mut results = Vec::new();

        settle_if_allowed(&epoch, &mut mode, ConstraintTransition::ActivateLocked(7));
        epoch.begin(false);
        record_motion(&epoch, mode, 60.0, &mut results);

        assert_eq!(results, vec![(Some(1), ConstraintMode::Locked(7), 60.0)]);
    }

    #[test]
    fn protocol_progression_cannot_change_generation_mid_batch() {
        let mut epoch = NativeInputEpoch::default();
        let mut mode = ConstraintMode::None;
        let mut results = Vec::new();
        epoch.begin(false);

        record_motion(&epoch, mode, 10.0, &mut results);
        settle_if_allowed(&epoch, &mut mode, ConstraintTransition::ActivateLocked(8));
        record_motion(&epoch, mode, 50.0, &mut results);
        epoch.finish(false);
        settle_if_allowed(&epoch, &mut mode, ConstraintTransition::ActivateLocked(8));

        assert_eq!(
            results,
            vec![
                (Some(1), ConstraintMode::None, 10.0),
                (Some(1), ConstraintMode::None, 50.0),
            ]
        );
        assert_eq!(mode, ConstraintMode::Locked(8));
    }

    #[test]
    fn protocol_progression_cannot_deactivate_mid_batch() {
        let mut epoch = NativeInputEpoch::default();
        let mut mode = ConstraintMode::Locked(9);
        let mut results = Vec::new();
        settle_if_allowed(&epoch, &mut mode, ConstraintTransition::ActivateLocked(9));
        epoch.begin(false);

        record_motion(&epoch, mode, 10.0, &mut results);
        settle_if_allowed(&epoch, &mut mode, ConstraintTransition::Deactivate);
        record_motion(&epoch, mode, 20.0, &mut results);
        epoch.finish(false);
        settle_if_allowed(&epoch, &mut mode, ConstraintTransition::Deactivate);
        epoch.begin(false);
        record_motion(&epoch, mode, 5.0, &mut results);

        assert_eq!(
            results,
            vec![
                (Some(1), ConstraintMode::Locked(9), 10.0),
                (Some(1), ConstraintMode::Locked(9), 20.0),
                (Some(2), ConstraintMode::None, 5.0),
            ]
        );
    }

    #[test]
    fn confined_transition_uses_the_same_epoch_boundary() {
        let mut epoch = NativeInputEpoch::default();
        let mut mode = ConstraintMode::None;
        let mut results = Vec::new();
        epoch.begin(false);

        record_motion(&epoch, mode, 10.0, &mut results);
        settle_if_allowed(
            &epoch,
            &mut mode,
            ConstraintTransition::ActivateConfined(11),
        );
        record_motion(&epoch, mode, 20.0, &mut results);
        epoch.finish(false);
        settle_if_allowed(
            &epoch,
            &mut mode,
            ConstraintTransition::ActivateConfined(11),
        );

        assert_eq!(
            results,
            vec![
                (Some(1), ConstraintMode::None, 10.0),
                (Some(1), ConstraintMode::None, 20.0),
            ]
        );
        assert_eq!(mode, ConstraintMode::Confined(11));
    }

    #[test]
    fn a_budget_continuation_keeps_one_constraint_epoch_open() {
        let mut epoch = NativeInputEpoch::default();
        let mut mode = ConstraintMode::None;
        epoch.begin(false);
        epoch.finish(true);
        assert!(epoch.backlog_pending());
        settle_if_allowed(&epoch, &mut mode, ConstraintTransition::ActivateLocked(12));
        let continuation = epoch.begin(true);
        assert_eq!(continuation, 1);
        assert_eq!(mode, ConstraintMode::None);
        epoch.finish(false);
        assert!(!epoch.backlog_pending());
        settle_if_allowed(&epoch, &mut mode, ConstraintTransition::ActivateLocked(12));
        assert_eq!(mode, ConstraintMode::Locked(12));
    }

    #[test]
    fn ordinary_pointer_motion_never_enters_full_server_progression() {
        for _ in 0..1_000 {
            assert!(!input_requires_full_server_progression(false, false));
            assert!(!input_requires_full_server_progression(true, false));
        }
    }

    #[test]
    fn constraint_sensitive_input_keeps_its_narrow_follow_up() {
        assert!(input_requires_full_server_progression(false, true));
        assert!(!input_requires_full_server_progression(true, true));
    }

    #[test]
    fn late_input_at_wayland_pre_read_cut_promotes_one_epoch_before_read() {
        let mut order = Vec::new();

        assert_eq!(
            decide_native_pre_read_input(true, false, true),
            NativePreReadInputDecision::PromoteInputEpoch,
        );
        order.push("pre_read_promote");
        order.push("native_epoch");
        order.push("wayland_read");

        assert_eq!(
            order,
            vec!["pre_read_promote", "native_epoch", "wayland_read"]
        );
    }

    #[test]
    fn pre_read_gate_is_skipped_for_existing_input_ownership() {
        assert_eq!(
            decide_native_pre_read_input(true, true, true),
            NativePreReadInputDecision::NoGate
        );
        assert_eq!(
            decide_native_pre_read_input(false, true, true),
            NativePreReadInputDecision::NoGate
        );
        assert_eq!(
            decide_native_pre_read_input(true, false, false),
            NativePreReadInputDecision::ReadWayland
        );
    }

    #[test]
    fn production_pre_read_seam_promotes_input_before_wayland_read() {
        let mut service_input = false;
        let mut order = Vec::new();

        assert_eq!(
            promote_native_input_before_wayland_read(true, &mut service_input, || {
                order.push("pre_read_probe");
                Ok::<bool, std::convert::Infallible>(true)
            },)
            .unwrap(),
            NativePreReadInputDecision::PromoteInputEpoch
        );
        assert!(service_input);
        order.push("native_epoch");
        order.push("wayland_read");

        assert_eq!(
            order,
            vec!["pre_read_probe", "native_epoch", "wayland_read"]
        );
    }

    #[test]
    fn pre_read_seam_reads_wayland_when_input_remains_unavailable() {
        let probe_count = std::cell::Cell::new(0);
        let mut service_input = false;

        let decision = promote_native_input_before_wayland_read(true, &mut service_input, || {
            probe_count.set(probe_count.get() + 1);
            Ok::<bool, std::convert::Infallible>(false)
        })
        .unwrap();

        assert_eq!(decision, NativePreReadInputDecision::ReadWayland);
        assert!(!service_input);
        assert_eq!(probe_count.get(), 1);
    }

    #[test]
    fn pre_read_seam_performs_at_most_one_probe() {
        let probe_count = std::cell::Cell::new(0);
        let mut service_input = false;

        let decision = promote_native_input_before_wayland_read(true, &mut service_input, || {
            probe_count.set(probe_count.get() + 1);
            Ok::<bool, std::convert::Infallible>(true)
        })
        .unwrap();

        assert_eq!(decision, NativePreReadInputDecision::PromoteInputEpoch);
        assert!(service_input);
        assert_eq!(probe_count.get(), 1);
    }

    #[test]
    fn promoted_pre_read_turn_cannot_probe_again_before_wayland_progress() {
        let probe_count = std::cell::Cell::new(0);
        let mut service_input = false;

        let first = promote_native_input_before_wayland_read(true, &mut service_input, || {
            probe_count.set(probe_count.get() + 1);
            Ok::<bool, std::convert::Infallible>(true)
        })
        .unwrap();
        let second = promote_native_input_before_wayland_read(true, &mut service_input, || {
            probe_count.set(probe_count.get() + 1);
            Ok::<bool, std::convert::Infallible>(true)
        })
        .unwrap();

        assert_eq!(first, NativePreReadInputDecision::PromoteInputEpoch);
        assert_eq!(second, NativePreReadInputDecision::NoGate);
        assert_eq!(probe_count.get(), 1);
    }

    #[test]
    fn pre_read_seam_does_not_probe_input_owned_turns() {
        let probe_count = std::cell::Cell::new(0);
        let mut service_input = true;

        let decision = promote_native_input_before_wayland_read(true, &mut service_input, || {
            probe_count.set(probe_count.get() + 1);
            Ok::<bool, std::convert::Infallible>(true)
        })
        .unwrap();

        assert_eq!(decision, NativePreReadInputDecision::NoGate);
        assert_eq!(probe_count.get(), 0);
    }

    #[test]
    fn pre_read_seam_does_not_probe_input_only_turns() {
        let probe_count = std::cell::Cell::new(0);
        let mut service_input = false;

        let decision = promote_native_input_before_wayland_read(false, &mut service_input, || {
            probe_count.set(probe_count.get() + 1);
            Ok::<bool, std::convert::Infallible>(true)
        })
        .unwrap();

        assert_eq!(decision, NativePreReadInputDecision::NoGate);
        assert!(!service_input);
        assert_eq!(probe_count.get(), 0);
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

fn keyboard_layout_argument_failure(id: u64) -> ControlResponse {
    ControlResponse::failure(
        id,
        ControlError::new(
            ControlErrorCode::InvalidArgument,
            "invalid keyboard layout arguments",
        )
        .with_detail("invalid_keyboard_layout_arguments"),
    )
}

fn keyboard_layout_failure(
    id: u64,
    error: oblivion_one::compositor::KeyboardLayoutControlError,
) -> ControlResponse {
    match error {
        oblivion_one::compositor::KeyboardLayoutControlError::InvalidIndex { index, count } => {
            ControlResponse::failure(
                id,
                ControlError::new(
                    ControlErrorCode::InvalidArgument,
                    format!("keyboard layout index {index} is out of range for {count} layouts"),
                )
                .with_detail("invalid_keyboard_layout_index"),
            )
        }
        oblivion_one::compositor::KeyboardLayoutControlError::Unavailable(_) => {
            ControlResponse::failure(
                id,
                ControlError::new(ControlErrorCode::Internal, "keyboard state unavailable")
                    .with_detail("keyboard_state_unavailable"),
            )
        }
        oblivion_one::compositor::KeyboardLayoutControlError::Internal(message) => {
            ControlResponse::failure(id, ControlError::new(ControlErrorCode::Internal, message))
        }
    }
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

#[derive(Debug, Clone, Copy)]
enum TriggerLivenessPoint {
    Event(usize),
    BatchEnd,
}

fn reconcile_trigger_liveness(
    server: &mut OwnCompositorServer,
    input_state: &NativeInputState,
    point: TriggerLivenessPoint,
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
            let after_event = match point {
                TriggerLivenessPoint::Event(event_index) => format!("event_index={event_index}"),
                TriggerLivenessPoint::BatchEnd => "batch_end".to_string(),
            };
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
