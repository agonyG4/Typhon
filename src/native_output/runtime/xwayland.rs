use super::*;
use std::collections::HashSet;

use oblivion_one::xwayland::X11WindowHandle;
use oblivion_one::xwayland::trace::{self, TraceFields};
use oblivion_one::xwayland::xwm::{XwmCommand, XwmEvent};

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

#[derive(Debug, Default)]
struct DestroyedCommandNormalization {
    commands: Vec<XwmCommand>,
    pruned_commands: usize,
    pruned_handles: usize,
}

fn normalize_destroyed_xwayland_commands(
    commands: Vec<XwmCommand>,
    destroyed: &HashSet<X11WindowHandle>,
) -> DestroyedCommandNormalization {
    let mut normalized = Vec::with_capacity(commands.len());
    let mut pruned_commands = 0;
    let mut pruned_handles = 0;
    for command in commands {
        let command = match command {
            XwmCommand::Map(handle)
            | XwmCommand::Unmap(handle)
            | XwmCommand::Configure { window: handle, .. }
            | XwmCommand::ConfigureFrame { window: handle, .. }
            | XwmCommand::ConfigureNotify { window: handle, .. }
            | XwmCommand::Raise(handle)
            | XwmCommand::Close(handle)
            | XwmCommand::SetState { window: handle, .. }
            | XwmCommand::BeginResizeSync { window: handle, .. }
            | XwmCommand::SetAllowCommits { window: handle, .. }
            | XwmCommand::ReleaseResizeCommits { window: handle, .. }
            | XwmCommand::CompleteResizeSync(handle)
                if destroyed.contains(&handle) =>
            {
                pruned_commands += 1;
                pruned_handles += 1;
                continue;
            }
            XwmCommand::Focus {
                window: Some(handle),
                ..
            } if destroyed.contains(&handle) => {
                pruned_commands += 1;
                pruned_handles += 1;
                continue;
            }
            XwmCommand::Stack { window, .. } if destroyed.contains(&window) => {
                pruned_commands += 1;
                pruned_handles += 1;
                continue;
            }
            XwmCommand::Stack {
                window,
                sibling,
                mode,
            } => {
                let sibling_was_pruned = sibling.is_some_and(|handle| destroyed.contains(&handle));
                pruned_handles += usize::from(sibling_was_pruned);
                XwmCommand::Stack {
                    window,
                    sibling: sibling.filter(|handle| !destroyed.contains(handle)),
                    mode,
                }
            }
            XwmCommand::RaiseAndSync {
                window,
                client_list,
                stacking,
            } => {
                let (client_list, client_pruned) = prune_destroyed_handles(client_list, destroyed);
                let (stacking, stacking_pruned) = prune_destroyed_handles(stacking, destroyed);
                pruned_handles += client_pruned + stacking_pruned;
                if destroyed.contains(&window) {
                    pruned_commands += 1;
                    pruned_handles += 1;
                    XwmCommand::SyncClientLists {
                        client_list,
                        stacking,
                    }
                } else {
                    XwmCommand::RaiseAndSync {
                        window,
                        client_list,
                        stacking,
                    }
                }
            }
            XwmCommand::RestackExact {
                order,
                client_list,
                stacking,
            } => {
                let (order, order_pruned) = prune_destroyed_handles(order, destroyed);
                let (client_list, client_pruned) = prune_destroyed_handles(client_list, destroyed);
                let (stacking, stacking_pruned) = prune_destroyed_handles(stacking, destroyed);
                pruned_handles += order_pruned + client_pruned + stacking_pruned;
                XwmCommand::RestackExact {
                    order,
                    client_list,
                    stacking,
                }
            }
            XwmCommand::RaiseFamily { family } => {
                let (family, family_pruned) = prune_destroyed_handles(family, destroyed);
                pruned_handles += family_pruned;
                XwmCommand::RaiseFamily { family }
            }
            XwmCommand::StackFamily { family, mode } => {
                let (family, family_pruned) = prune_destroyed_handles(family, destroyed);
                pruned_handles += family_pruned;
                XwmCommand::StackFamily { family, mode }
            }
            XwmCommand::SyncClientLists {
                client_list,
                stacking,
            } => {
                let (client_list, client_pruned) = prune_destroyed_handles(client_list, destroyed);
                let (stacking, stacking_pruned) = prune_destroyed_handles(stacking, destroyed);
                pruned_handles += client_pruned + stacking_pruned;
                XwmCommand::SyncClientLists {
                    client_list,
                    stacking,
                }
            }
            command => command,
        };
        normalized.push(command);
    }
    DestroyedCommandNormalization {
        commands: normalized,
        pruned_commands,
        pruned_handles,
    }
}

fn prune_destroyed_handles(
    handles: Vec<X11WindowHandle>,
    destroyed: &HashSet<X11WindowHandle>,
) -> (Vec<X11WindowHandle>, usize) {
    let mut pruned_handles = 0;
    let handles = handles
        .into_iter()
        .filter(|handle| {
            let keep = !destroyed.contains(handle);
            pruned_handles += usize::from(!keep);
            keep
        })
        .collect();
    (handles, pruned_handles)
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
        let translation_started = std::time::Instant::now();
        let mut commands = Vec::new();
        let mut destroyed = HashSet::new();
        let mut events_processed = 0;
        for event in self.xwayland.take_managed_xwm_events() {
            events_processed += 1;
            if let XwmEvent::WindowDestroyed(handle) = &event {
                destroyed.insert(*handle);
            }
            trace::emit("xwm_event_dispatched", || {
                TraceFields::new()
                    .field("source", "native_runtime")
                    .field("event", format!("{event:?}"))
            });
            commands.extend(self.server.apply_xwayland_window_event(event));
        }
        let now_ns = monotonic_now_ns()?;
        commands.extend(self.server.take_xwayland_backend_commands(now_ns));
        let normalization = normalize_destroyed_xwayland_commands(commands, &destroyed);
        if normalization.pruned_commands > 0 || normalization.pruned_handles > 0 {
            let sample_terminal = destroyed.iter().next().copied();
            self.xwayland.note_runtime_xwayland_same_batch_pruning(
                destroyed.len(),
                sample_terminal.map(|handle| handle.xid()),
                sample_terminal.map(|handle| handle.generation().get()),
                normalization.pruned_commands,
                normalization.pruned_handles,
            );
        }
        let translation_us = translation_started.elapsed().as_micros() as u64;
        let execution_started = std::time::Instant::now();
        let commands = coalesce_client_list_sync(normalization.commands);
        let command_count = commands.len();
        for command in commands {
            let _ = self
                .xwayland
                .execute_managed_command(&mut self.process_supervisor, command);
        }
        self.xwayland.note_runtime_xwayland_timing(
            events_processed,
            command_count,
            translation_us,
            execution_started.elapsed().as_micros() as u64,
        );
        let _ = self
            .xwayland
            .flush_managed_commands(&mut self.process_supervisor);
        Ok(())
    }

    pub(super) fn dispatch_xwayland_buffer_ready(&mut self) {
        for (generation, surface_id) in self.server.take_xwayland_buffer_level_events() {
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
        for event in self.server.take_xwayland_buffer_ready_events() {
            trace::emit("commit_observed_dispatched", || {
                TraceFields::new()
                    .field("source", "native_runtime")
                    .field("generation", event.generation.get())
                    .field("surface_id", event.surface_id)
                    .field("association_serial", event.association_serial.get())
                    .field("commit_sequence", event.commit_sequence.get())
            });
            let _ = self
                .xwayland
                .mark_managed_surface_commit_observed(&mut self.process_supervisor, event);
        }
    }

    pub(super) fn dispatch_xwayland_association_events(&mut self) {
        for event in self.xwayland.take_managed_association_events() {
            trace::emit("xwm_association_event_dispatched", || {
                TraceFields::new()
                    .field("source", "native_runtime")
                    .field("event", format!("{event:?}"))
            });
            self.server.apply_xwayland_association_event(event);
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

    #[test]
    fn same_batch_destroy_prunes_obsolete_commands_without_reordering_live_work() {
        let generation = oblivion_one::xwayland::XwaylandGeneration::new(
            std::num::NonZeroU64::new(1).expect("nonzero generation"),
        );
        let dead = X11WindowHandle::new(generation, 10);
        let live = X11WindowHandle::new(generation, 11);
        let destroyed = HashSet::from([dead]);

        let normalization = normalize_destroyed_xwayland_commands(
            vec![
                XwmCommand::Configure {
                    window: dead,
                    geometry: Default::default(),
                    fields: Default::default(),
                    border_width: 0,
                },
                XwmCommand::SetState {
                    window: dead,
                    state: Default::default(),
                },
                XwmCommand::Stack {
                    window: live,
                    sibling: Some(dead),
                    mode: oblivion_one::xwayland::xwm::X11StackMode::Above,
                },
                XwmCommand::SyncClientLists {
                    client_list: vec![dead, live],
                    stacking: vec![live, dead],
                },
            ],
            &destroyed,
        );

        assert_eq!(
            normalization.commands,
            vec![
                XwmCommand::Stack {
                    window: live,
                    sibling: None,
                    mode: oblivion_one::xwayland::xwm::X11StackMode::Above,
                },
                XwmCommand::SyncClientLists {
                    client_list: vec![live],
                    stacking: vec![live],
                },
            ]
        );
        assert_eq!(normalization.pruned_commands, 2);
        assert_eq!(normalization.pruned_handles, 5);
    }
}
