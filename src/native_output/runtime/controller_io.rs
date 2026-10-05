use super::*;

use oblivion_one::control_snapshots::ControllerObserverSnapshot;

impl NativeRuntime {
    pub(super) fn sync_controller_reactor_sources(&mut self) -> NativeResult<()> {
        let desired_monitor_fd = self
            .controller_manager
            .as_ref()
            .and_then(ControllerManager::monitor_fd);
        match (self.controller_monitor_reactor_token, desired_monitor_fd) {
            (Some(token), None) => {
                self.event_loop.unregister(token)?;
                self.controller_monitor_reactor_token = None;
            }
            (None, Some(fd)) => match self
                .event_loop
                .register(fd, NativeEventSource::ControllerMonitor)
            {
                Ok(token) => self.controller_monitor_reactor_token = Some(token),
                Err(error) => {
                    eprintln!("native controller monitor registration failed: {error}");
                    if let Some(manager) = self.controller_manager.as_mut() {
                        manager.disable_monitor();
                    }
                }
            },
            _ => {}
        }

        loop {
            let stale_id = self
                .controller_device_reactor_tokens
                .keys()
                .copied()
                .find(|id| {
                    self.controller_manager
                        .as_ref()
                        .is_none_or(|manager| !manager.contains_id(*id))
                });
            let Some(id) = stale_id else {
                break;
            };
            if let Some(token) = self.controller_device_reactor_tokens.remove(&id) {
                self.event_loop.unregister(token)?;
            }
        }

        let mut failed_ids = [None; MAX_CONTROLLER_DEVICES];
        let mut failed_count = 0usize;
        if let Some(manager) = self.controller_manager.as_ref() {
            for id in manager.device_ids() {
                if self.controller_device_reactor_tokens.contains_key(&id) {
                    continue;
                }
                let Some(fd) = manager.device_fd(id) else {
                    continue;
                };
                match self
                    .event_loop
                    .register(fd, NativeEventSource::ControllerDevice(id.get()))
                {
                    Ok(token) => {
                        self.controller_device_reactor_tokens.insert(id, token);
                    }
                    Err(error) => {
                        eprintln!("native controller device registration failed: {error}");
                        if failed_count < failed_ids.len() {
                            failed_ids[failed_count] = Some(id);
                            failed_count += 1;
                        }
                    }
                }
            }
        }
        if let Some(manager) = self.controller_manager.as_mut() {
            for id in failed_ids.into_iter().take(failed_count).flatten() {
                manager.record_registration_failure(true);
                manager.retire_device(id);
            }
        }
        Ok(())
    }

    pub(super) fn suspend_controller_observation(&mut self) -> NativeResult<()> {
        if let Some(manager) = self.controller_manager.as_mut() {
            manager.suspend();
        }
        if let Some(token) = self.controller_monitor_reactor_token.take() {
            self.event_loop.unregister(token)?;
        }
        for token in self
            .controller_device_reactor_tokens
            .drain()
            .map(|(_, token)| token)
        {
            self.event_loop.unregister(token)?;
        }
        Ok(())
    }

    pub(super) fn resume_controller_observation(&mut self) -> NativeResult<()> {
        if let Some(manager) = self.controller_manager.as_mut() {
            manager.resume();
        }
        self.sync_controller_reactor_sources()
    }

    pub(super) fn service_controller_work(&mut self, cycle: &NativeCycleState) -> NativeResult<()> {
        let backlog_requested = cycle
            .wakeup
            .continuation
            .contains(NativeContinuationReason::ControllerBacklog);
        let readiness = &cycle.wakeup.controller_device_events;
        let token_map = &self.controller_device_reactor_tokens;
        let Some(manager) = self.controller_manager.as_mut() else {
            return Ok(());
        };

        if cycle.wakeup.controller_monitor_ready {
            manager.service_monitor();
        }
        let outcome = manager.drain_ready(readiness, |id, token| {
            token_map
                .get(&id)
                .is_some_and(|registered| *registered == token)
        });
        if backlog_requested {
            manager.record_backlog_continuation();
        }
        if outcome.meaningful_activity {
            self.server.notify_user_activity();
        }
        if outcome.backlog_pending {
            self.request_native_continuation(NativeContinuationReason::ControllerBacklog)?;
        }
        self.sync_controller_reactor_sources()
    }

    pub(super) fn controller_observer_snapshot(&self) -> ControllerObserverSnapshot {
        let Some(manager) = self.controller_manager.as_ref() else {
            return ControllerObserverSnapshot::default();
        };
        let telemetry = manager.telemetry();
        ControllerObserverSnapshot {
            policy: manager.policy().as_str().to_string(),
            connected_devices: u32::try_from(manager.connected_count()).unwrap_or(u32::MAX),
            device_adds: telemetry.device_adds,
            device_removes: telemetry.device_removes,
            raw_events: telemetry.raw_events,
            logical_frames: telemetry.logical_frames,
            activity_transitions: telemetry.activity_transitions,
            drain_budget_exhaustions: telemetry.drain_budget_exhaustions,
            backlog_continuations: telemetry.backlog_continuations,
            // evdev's synchronized fetch path repairs SYN_DROPPED internally
            // but does not expose a recovery counter.
            synchronization_recoveries: None,
            read_failures: telemetry.read_failures,
            hotplug_failures: telemetry.hotplug_failures,
        }
    }
}
