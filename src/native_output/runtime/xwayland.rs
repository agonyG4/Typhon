use super::*;
use oblivion_one::xwayland::trace::{self, TraceFields};
use oblivion_one::xwayland::xwm::XwmCommand;

fn coalesce_client_list_sync(commands: Vec<XwmCommand>) -> Vec<XwmCommand> {
    let mut normalized = Vec::with_capacity(commands.len());
    let mut latest_snapshot = None;
    for command in commands {
        match command {
            command @ XwmCommand::SyncClientLists { .. } => latest_snapshot = Some(command),
            command => normalized.push(command),
        }
    }
    normalized.extend(latest_snapshot);
    normalized
}

impl NativeRuntime {
    pub(super) fn initialize_managed_xwayland(&mut self) -> NativeResult<()> {
        if !self.xwayland.is_managed()
            || self.xwayland.state_kind() != oblivion_one::xwayland::XwaylandStateKind::Starting
        {
            return Ok(());
        }
        let Some(generation) = self.xwayland.generation() else {
            return Ok(());
        };
        match self
            .xwayland
            .initialize_managed_xwm(generation, &mut self.process_supervisor)
        {
            Ok(()) => self.sync_xwayland_reactor_sources(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
            Err(error) => {
                eprintln!("native XWayland managed startup contained: {error}");
                self.sync_xwayland_reactor_sources()
            }
        }
    }

    pub(super) fn dispatch_xwayland_window_events(&mut self) -> NativeResult<()> {
        let mut commands = Vec::new();
        for event in self.xwayland.take_managed_xwm_events() {
            trace::emit("xwm_event_dispatched", || {
                TraceFields::new()
                    .field("source", "native_runtime")
                    .field("event", format!("{event:?}"))
            });
            commands.extend(self.server.apply_xwayland_window_event(event));
        }
        let now_ns = monotonic_now_ns()?;
        commands.extend(self.server.take_xwayland_backend_commands(now_ns));
        for command in coalesce_client_list_sync(commands) {
            let _ = self
                .xwayland
                .execute_managed_command(&mut self.process_supervisor, command);
        }
        let _ = self
            .xwayland
            .flush_managed_commands(&mut self.process_supervisor);
        Ok(())
    }

    pub(super) fn dispatch_xwayland_buffer_ready(&mut self) {
        for (generation, surface_id) in self.server.take_xwayland_buffer_ready_events() {
            trace::emit("buffer_ready_event_dispatched", || {
                TraceFields::new()
                    .field("source", "native_runtime")
                    .field("generation", generation.get())
                    .field("surface_id", surface_id)
            });
            let _ = self.xwayland.mark_managed_surface_buffer_ready(
                &mut self.process_supervisor,
                generation,
                surface_id,
            );
        }
    }

    pub(super) fn reap_supervised_children(
        &mut self,
        cycle: &NativeCycleState,
    ) -> NativeResult<()> {
        self.astrea_launch_tracker.prune_dead();
        if !cycle.wakeup.reasons.child_signal()
            && self.shutdown.state() != ShutdownState::StoppingChildren
        {
            return Ok(());
        }
        for exit in self.process_supervisor.reap_exited()? {
            let xwayland_exit = self.xwayland.handle_process_exit(&exit)?;
            if xwayland_exit {
                self.revoke_xwayland_private_client();
            }
            let finished_status = astrea_launch_finished_status(exit.status);
            self.perf.log("process.exit", || {
                vec![
                    NativePerfField::str("kind", exit.kind.as_str()),
                    NativePerfField::u64("pid", u64::from(exit.pid)),
                    NativePerfField::str("exit_code", finished_status.to_string()),
                    NativePerfField::u64("restarted_pid", exit.restarted_pid.map_or(0, u64::from)),
                ]
            });
            if self.astrea_launch_tracker.complete(exit.pid, exit.status) {
                self.perf.log("shell_control.finished", || {
                    vec![
                        NativePerfField::u64("pid", u64::from(exit.pid)),
                        NativePerfField::str("status", finished_status.to_string()),
                    ]
                });
            }
        }
        Ok(())
    }

    pub(super) fn dispatch_xwayland_shell_binds(&mut self) -> NativeResult<()> {
        for identity in self.server.take_xwayland_shell_bind_events() {
            self.xwayland
                .handle_shell_bind_for_client(identity.generation, &identity.client_id)?;
        }
        Ok(())
    }

    pub(super) fn dispatch_xwayland_client_disconnects(&mut self) -> NativeResult<()> {
        for identity in self.server.take_xwayland_client_disconnect_events() {
            if self.xwayland_client_identity.as_ref() != Some(&identity) {
                self.xwayland.record_stale_reactor_event();
                continue;
            }
            self.xwayland_client_identity = None;
            self.xwayland.handle_private_client_disconnected(
                identity.generation,
                &mut self.process_supervisor,
            )?;
        }
        Ok(())
    }

    pub(super) fn dispatch_xwayland_events(&mut self, wakeup: &NativeWakeup) -> NativeResult<()> {
        let mut started_generation = None;
        for event in wakeup.xwayland_events.iter().copied() {
            let Some((_, registration)) = self
                .xwayland_reactor_tokens
                .iter()
                .find(|(token, _)| *token == event.token)
                .copied()
            else {
                self.xwayland.record_stale_reactor_event();
                eprintln!(
                    "oblivion-one xwayland: event=xwm_reactor_rejected reason=missing_registration"
                );
                continue;
            };
            let continuation = match self.xwayland.handle_reactor_event_with_token(
                registration.purpose,
                registration.generation,
                event.flags,
                event.token.raw(),
                &mut self.process_supervisor,
            ) {
                Ok(continuation) => continuation,
                Err(error) => {
                    eprintln!(
                        "native XWayland event contained generation={:?} purpose={:?}: {error}",
                        registration.generation, registration.purpose
                    );
                    false
                }
            };
            if continuation
                && matches!(
                    registration.purpose,
                    XwaylandReactorPurpose::ListenFilesystem
                        | XwaylandReactorPurpose::ListenAbstract
                )
            {
                started_generation = self.xwayland.generation();
            }
            if continuation {
                self.event_loop.arm_deadline(Some(monotonic_now_ns()?))?;
            }
        }
        self.sync_xwayland_reactor_sources()?;
        if let Some(generation) = started_generation {
            if let Err(error) = self
                .xwayland
                .probe_displayfd(generation, &mut self.process_supervisor)
            {
                eprintln!(
                    "native XWayland displayfd probe contained generation={generation:?}: {error}"
                );
            }
            self.sync_xwayland_reactor_sources()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesce_client_list_sync_keeps_only_final_snapshot_after_other_commands() {
        let final_snapshot = XwmCommand::SyncClientLists {
            client_list: Vec::new(),
            stacking: Vec::new(),
        };

        let commands = coalesce_client_list_sync(vec![
            XwmCommand::SyncClientLists {
                client_list: Vec::new(),
                stacking: Vec::new(),
            },
            XwmCommand::Focus {
                window: None,
                timestamp: 42,
            },
            final_snapshot.clone(),
        ]);

        assert_eq!(
            commands,
            vec![
                XwmCommand::Focus {
                    window: None,
                    timestamp: 42,
                },
                final_snapshot,
            ]
        );
    }

    #[test]
    fn coalesce_client_list_sync_leaves_non_snapshot_batch_unchanged() {
        let commands = vec![XwmCommand::Focus {
            window: None,
            timestamp: 7,
        }];

        assert_eq!(coalesce_client_list_sync(commands.clone()), commands,);
    }
}
