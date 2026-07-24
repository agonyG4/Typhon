use std::io;

use crate::process::ChildSupervisor;

use super::trace::{self, TraceFields};
use super::{PendingTermination, ServiceState, XwaylandFailureStage, XwaylandService};
use super::{XwmCommand, XwmCommandOutcome};

impl XwaylandService {
    pub fn execute_managed_command(
        &mut self,
        supervisor: &mut ChildSupervisor,
        command: XwmCommand,
    ) -> io::Result<()> {
        self.execute_managed_command_with_context(supervisor, command, false)
    }

    pub fn execute_managed_command_with_context(
        &mut self,
        supervisor: &mut ChildSupervisor,
        command: XwmCommand,
        same_batch_terminal: bool,
    ) -> io::Result<()> {
        let command_kind = command.kind_name();
        let primary_xid = command.primary_handle().map(|handle| handle.xid());
        let generation = command
            .primary_handle()
            .map(|handle| handle.generation().get())
            .or_else(|| self.generation().map(|generation| generation.get()));
        let command_debug = format!("{command:?}");
        if matches!(command, XwmCommand::BeginResizeSync { .. }) {
            self.metrics.resize_sync_started = self.metrics.resize_sync_started.saturating_add(1);
        }
        let result = match &mut self.state {
            ServiceState::Running(resources) => match resources.xwm.execute(command) {
                Ok(outcome) => {
                    match outcome {
                        XwmCommandOutcome::Applied => {
                            self.metrics.xwm_commands_applied =
                                self.metrics.xwm_commands_applied.saturating_add(1);
                            trace::emit("xwm_command_outcome", || {
                                TraceFields::new()
                                    .field("command_kind", command_kind)
                                    .field("primary_xid", primary_xid.unwrap_or(0))
                                    .field("generation", generation.unwrap_or(0))
                                    .field("outcome", "applied")
                                    .field("pruned_handle_count", 0)
                                    .field("same_batch_terminal", same_batch_terminal)
                            });
                        }
                        XwmCommandOutcome::DroppedTargetGone { window } => {
                            self.metrics.xwm_commands_dropped_target_gone = self
                                .metrics
                                .xwm_commands_dropped_target_gone
                                .saturating_add(1);
                            trace::emit("xwm_command_outcome", || {
                                TraceFields::new()
                                    .field("command_kind", command_kind)
                                    .field("primary_xid", window.xid())
                                    .field("generation", window.generation().get())
                                    .field("outcome", "dropped_target_gone")
                                    .field("pruned_handle_count", 0)
                                    .field("same_batch_terminal", same_batch_terminal)
                            });
                        }
                        XwmCommandOutcome::DroppedStaleGeneration { window } => {
                            self.metrics.xwm_commands_dropped_stale_generation = self
                                .metrics
                                .xwm_commands_dropped_stale_generation
                                .saturating_add(1);
                            trace::emit("xwm_command_outcome", || {
                                TraceFields::new()
                                    .field("command_kind", command_kind)
                                    .field(
                                        "primary_xid",
                                        window.map(|handle| handle.xid()).unwrap_or(0),
                                    )
                                    .field("generation", generation.unwrap_or(0))
                                    .field("outcome", "dropped_stale_generation")
                                    .field("pruned_handle_count", 0)
                                    .field("same_batch_terminal", same_batch_terminal)
                            });
                        }
                        XwmCommandOutcome::AppliedAfterPruning { dropped_handles } => {
                            self.metrics.xwm_commands_applied =
                                self.metrics.xwm_commands_applied.saturating_add(1);
                            self.metrics.xwm_command_list_handles_pruned = self
                                .metrics
                                .xwm_command_list_handles_pruned
                                .saturating_add(dropped_handles as u64);
                            trace::emit("xwm_command_outcome", || {
                                TraceFields::new()
                                    .field("command_kind", command_kind)
                                    .field("primary_xid", primary_xid.unwrap_or(0))
                                    .field("generation", generation.unwrap_or(0))
                                    .field("outcome", "applied_after_pruning")
                                    .field("pruned_handle_count", dropped_handles)
                                    .field("same_batch_terminal", same_batch_terminal)
                            });
                        }
                    }
                    None
                }
                Err(error) => Some(io::Error::other(format!(
                    "{error}; command_kind={command_kind}; primary_xid={}; generation={}; same_batch_terminal={same_batch_terminal}; command={command_debug}",
                    primary_xid.unwrap_or(0),
                    generation.unwrap_or(0),
                ))),
            },
            _ => None,
        };
        if let Some(error) = result {
            self.metrics.xwm_command_failures_fatal =
                self.metrics.xwm_command_failures_fatal.saturating_add(1);
            if command_kind == "BeginResizeSync" {
                self.metrics.resize_sync_command_failures =
                    self.metrics.resize_sync_command_failures.saturating_add(1);
            }
            self.fail_managed_xwm(supervisor, XwaylandFailureStage::CommandWrite, error);
        }
        Ok(())
    }

    pub fn handle_focus_deadline(
        &mut self,
        now_ns: u64,
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<()> {
        let focus_error = if let ServiceState::Running(resources) = &mut self.state {
            resources
                .xwm
                .handle_focus_deadline(now_ns)
                .err()
                .map(io::Error::other)
        } else {
            None
        };
        if let Some(error) = focus_error {
            self.fail_managed_xwm(supervisor, XwaylandFailureStage::CommandFlush, error);
        }
        Ok(())
    }

    pub fn handle_deadline(
        &mut self,
        now_ns: u64,
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<()> {
        if let Some(pending) = self.pending_termination {
            if !supervisor.contains_id(pending.process_id) {
                self.pending_termination = None;
            } else if !pending.escalated && now_ns >= pending.deadline_ns {
                self.kill_process_now(supervisor, pending.process_id)?;
                self.pending_termination = Some(PendingTermination {
                    escalated: true,
                    ..pending
                });
            }
        }
        let (adoption_metrics, timeout_summary, resize_sync_error) =
            if let ServiceState::Running(resources) = &mut self.state {
                let adoption_started = std::time::Instant::now();
                let timeout_summary = resources.xwm.collect_adoption_expirations(now_ns);
                let adoption_metrics = resources.xwm.adoption_metrics();
                let resize_sync_error = resources
                    .xwm
                    .handle_resize_sync_deadline(now_ns)
                    .err()
                    .map(io::Error::other);
                self.metrics.adoption_deadline_max_us = self.metrics.adoption_deadline_max_us.max(
                    adoption_started
                        .elapsed()
                        .as_micros()
                        .min(u128::from(u64::MAX)) as u64,
                );
                (Some(adoption_metrics), timeout_summary, resize_sync_error)
            } else {
                (None, false, None)
            };
        if let Some(adoption_metrics) = adoption_metrics {
            self.metrics.adoption_waits_started = adoption_metrics.waits_started;
            self.metrics.adoption_waits_completed = adoption_metrics.waits_completed;
            self.metrics.adoption_waits_cancelled_unmap = adoption_metrics.waits_cancelled_unmap;
            self.metrics.adoption_waits_cancelled_destroy =
                adoption_metrics.waits_cancelled_destroy;
            self.metrics.adoption_waits_expired = adoption_metrics.waits_expired;
            self.metrics.adoption_peak_pending = adoption_metrics.peak_pending;
            if timeout_summary {
                self.metrics.adoption_timeout_summaries =
                    self.metrics.adoption_timeout_summaries.saturating_add(1);
            }
        }
        if let Some(error) = resize_sync_error {
            self.fail_managed_xwm(supervisor, XwaylandFailureStage::CommandFlush, error);
        }
        let startup_timed_out = matches!(
            &self.state,
            ServiceState::Starting(resources) if now_ns >= resources.deadline_ns
        );
        if startup_timed_out {
            let (generation, process_id) = match &self.state {
                ServiceState::Starting(resources) => (resources.generation, resources.process.id),
                _ => unreachable!("startup timeout state changed before diagnostics"),
            };
            let process_alive = supervisor.contains_id(process_id);
            self.log_displayfd_event(
                "displayfd_probe",
                Some("timeout_final"),
                Some(generation),
                Some(process_id),
                self.displayfd_parent_fd(generation),
                self.displayfd_child_source_fd(generation),
                self.displayfd_reactor_token(generation),
                None,
                None,
            );
            if process_alive && let Err(error) = self.probe_displayfd(generation, supervisor) {
                eprintln!(
                    "oblivion-one xwayland: event=displayfd_final_probe_failed generation={generation:?} error={error}"
                );
            }
            if !matches!(
                &self.state,
                ServiceState::Starting(resources) if resources.generation == generation
            ) {
                return Ok(());
            }
            let mut readiness = match &self.state {
                ServiceState::Starting(resources) => self.snapshot_for_starting(resources),
                _ => unreachable!("startup timeout state changed after final probe"),
            };
            readiness.process_alive = process_alive;
            self.last_readiness = Some(readiness);
            eprintln!(
                "oblivion-one xwayland: event=readiness_timeout generation={:?} display={} process_id={} elapsed_ns={} process_alive={} displayfd_registered={} displayfd_readable={} display_number_validated={} private_wayland_endpoint_transferred={} private_client_attached={} private_client_authorized={} xwayland_shell_bound={} xwm_connected={} xwm_capabilities_validated={} root_initialized={} readiness_complete=false missing={:?}",
                readiness.generation,
                readiness.display,
                readiness.process_id.get(),
                readiness.elapsed_ns,
                readiness.process_alive,
                readiness.displayfd_registered,
                readiness.displayfd_readable,
                readiness.display_number_validated,
                readiness.private_wayland_endpoint_transferred,
                readiness.private_client_attached,
                readiness.private_client_authorized,
                readiness.xwayland_shell_bound,
                readiness.xwm_connected,
                readiness.xwm_capabilities_validated,
                readiness.root_initialized,
                readiness.missing_conditions(),
            );
            self.metrics.readiness_failures = self.metrics.readiness_failures.saturating_add(1);
            self.request_process_termination(supervisor, process_id)?;
            self.mark_stderr_failure();
            self.enter_failure_backoff(now_ns);
        } else if matches!(&self.state, ServiceState::Backoff { deadline_ns, .. } if now_ns >= *deadline_ns)
        {
            self.rearm(false);
        }
        Ok(())
    }
}
