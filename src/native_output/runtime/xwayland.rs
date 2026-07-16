use super::*;

impl NativeRuntime {
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

    pub(super) fn dispatch_xwayland_events(&mut self, wakeup: &NativeWakeup) -> NativeResult<()> {
        for event in wakeup.xwayland_events.iter().copied() {
            let Some((_, registration)) = self
                .xwayland_reactor_tokens
                .iter()
                .find(|(token, _)| *token == event.token)
                .copied()
            else {
                self.xwayland.record_stale_reactor_event();
                continue;
            };
            if let Err(error) = self.xwayland.handle_reactor_event(
                registration.purpose,
                registration.generation,
                event.flags,
                &mut self.process_supervisor,
            ) {
                eprintln!(
                    "native XWayland event contained generation={:?} purpose={:?}: {error}",
                    registration.generation, registration.purpose
                );
            }
        }
        self.sync_xwayland_reactor_sources()?;
        Ok(())
    }
}
