use super::cursor_cycle::apply_cursor_position;
use super::*;

impl NativeRuntime {
    #[allow(unused_variables)]
    pub(super) fn dispatch_wayland_and_input(
        &mut self,
        cycle: &mut NativeCycleState,
    ) -> NativeResult<()> {
        if cycle.wakeup.reasons.input() {
            NativeSessionIo::observe(self, NativeIoOperation::RawInputAction);
        }
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
            acquire_notifier,
            acquire_watches,
            parked_acquire_watches: _,
            event_loop,
            drm_reactor_token: _,
            frame_scheduler,
            effective_app_gpu_policy,
            last_renderable_surfaces,
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
            shutdown: _,
            session: _,
            ..
        } = self;
        let present_us = 0;
        let pageflip_pending_at_tick = scanout.page_flip_pending();
        let tick_start = Instant::now();
        let accepted = server.tick()?;
        let tick_us = elapsed_micros(tick_start);
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
        for (event_index, event) in coalesced_events.into_iter().enumerate() {
            let may_change_pointer_constraints = event.may_change_pointer_constraints();
            let effect = input_state.handle_hardware_input_event(event);
            let effect_requested_redraw = effect.redraw_requested;
            let cursor_visible = server.client_cursor_render_state().is_some()
                || server.interaction_cursor_override_active()
                || input_state.cursor_visible();
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
                    acquire_watches.shutdown(event_loop)?;
                    return Err(error.into());
                }
                return Err(error.into());
            }
            let application = apply_native_input_effect(
                effect,
                NativeInputApplyContext {
                    server,
                    perf,
                    resize_perf,
                    cursor_mode: *cursor_render_mode,
                    app_gpu_policy: *effective_app_gpu_policy,
                    seat_session: seat_session.as_ref(),
                    process_supervisor,
                },
            )?;
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
            if may_change_pointer_constraints {
                let _ = server.tick()?;
                redraw_requested |= process_native_pointer_constraint_backend_requests(
                    server,
                    pointer_constraint_backend,
                    input_state,
                    *cursor_render_mode,
                )?;
                synchronize_cursor_state_for_server(
                    server,
                    atomic_cursor,
                    legacy_cursor,
                    input_state,
                )?;
            }
        }
        let interaction_reconciled = reconcile_trigger_liveness(server, input_state, "batch_end");
        redraw_requested |= interaction_reconciled;
        redraw_requested |= process_native_pointer_constraint_backend_requests(
            server,
            pointer_constraint_backend,
            input_state,
            *cursor_render_mode,
        )?;
        synchronize_cursor_state_for_server(server, atomic_cursor, legacy_cursor, input_state)?;
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
