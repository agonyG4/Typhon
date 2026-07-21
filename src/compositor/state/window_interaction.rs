use super::*;

#[derive(Debug, Clone, Copy)]
pub(in crate::compositor) struct BeginWindowInteraction {
    pub(super) window_id: Option<WindowId>,
    pub(super) root_surface_id: u32,
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) kind: WindowInteractionKind,
    pub(super) source: WindowInteractionSource,
    pub(super) trigger_button: Option<u32>,
    pub(super) trigger_serial: Option<u32>,
    pub(super) pointer_motion_surface_id: Option<u32>,
}

impl CompositorState {
    pub(in crate::compositor) fn begin_window_move_at(&mut self, x: f64, y: f64) -> bool {
        self.begin_window_interaction_at(
            x,
            y,
            WindowInteractionKind::Move,
            WindowInteractionSource::NativeBinding,
            None,
            None,
        )
    }

    pub(in crate::compositor) fn begin_window_move_at_with_trigger(
        &mut self,
        x: f64,
        y: f64,
        trigger_button: u32,
    ) -> bool {
        self.begin_window_interaction_at(
            x,
            y,
            WindowInteractionKind::Move,
            WindowInteractionSource::NativeBinding,
            Some(trigger_button),
            None,
        )
    }

    pub(in crate::compositor) fn begin_window_resize_at(&mut self, x: f64, y: f64) -> bool {
        self.begin_window_resize_at_with_trigger(x, y, 0)
    }

    pub(in crate::compositor) fn begin_window_resize_at_with_trigger(
        &mut self,
        x: f64,
        y: f64,
        trigger_button: u32,
    ) -> bool {
        let Some(surface_id) = self.surface_id_at(x, y) else {
            log_begin_rejection_without_target(
                "no_surface_at_pointer",
                x,
                y,
                WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT),
                WindowInteractionSource::NativeBinding,
                trigger_button,
            );
            return false;
        };
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        let Some((local_x, local_y, width, height)) =
            self.root_window_local_point_at(root_surface_id, x, y)
        else {
            return self.begin_window_interaction_for_root(BeginWindowInteraction {
                window_id: self.window_id_for_surface(root_surface_id),
                root_surface_id,
                x,
                y,
                kind: WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT),
                source: WindowInteractionSource::NativeBinding,
                trigger_button: (trigger_button != 0).then_some(trigger_button),
                trigger_serial: None,
                pointer_motion_surface_id: Some(surface_id),
            });
        };
        let edges = resize_edges_for_window_point(local_x, local_y, width, height);
        self.begin_window_interaction_for_root(BeginWindowInteraction {
            window_id: self.window_id_for_surface(root_surface_id),
            root_surface_id,
            x,
            y,
            kind: WindowInteractionKind::Resize(edges),
            source: WindowInteractionSource::NativeBinding,
            trigger_button: (trigger_button != 0).then_some(trigger_button),
            trigger_serial: None,
            pointer_motion_surface_id: Some(surface_id),
        })
    }

    pub(in crate::compositor) fn begin_window_frame_action_at(&mut self, x: f64, y: f64) -> bool {
        let Some(hit) = self.window_frame_hit_at(x, y) else {
            log_begin_rejection_without_target(
                "no_surface_at_pointer",
                x,
                y,
                WindowInteractionKind::Move,
                WindowInteractionSource::NativeBinding,
                0,
            );
            return false;
        };
        self.begin_window_interaction_for_root(BeginWindowInteraction {
            window_id: Some(hit.window_id),
            root_surface_id: hit.root_surface_id,
            x,
            y,
            kind: hit.kind,
            source: WindowInteractionSource::NativeBinding,
            trigger_button: None,
            trigger_serial: None,
            pointer_motion_surface_id: None,
        })
    }

    pub(in crate::compositor) fn begin_window_interaction_at(
        &mut self,
        x: f64,
        y: f64,
        kind: WindowInteractionKind,
        source: WindowInteractionSource,
        trigger_button: Option<u32>,
        trigger_serial: Option<u32>,
    ) -> bool {
        let Some(surface_id) = self.surface_id_at(x, y) else {
            log_begin_rejection_without_target(
                "no_surface_at_pointer",
                x,
                y,
                kind,
                source,
                trigger_button.unwrap_or_default(),
            );
            return false;
        };
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        self.begin_window_interaction_for_root(BeginWindowInteraction {
            window_id: self.window_id_for_surface(root_surface_id),
            root_surface_id,
            x,
            y,
            kind,
            source,
            trigger_button,
            trigger_serial,
            pointer_motion_surface_id: Some(surface_id),
        })
    }

    pub(in crate::compositor) fn begin_client_window_move(
        &mut self,
        surface: &wl_surface::WlSurface,
        serial: u32,
    ) -> bool {
        let root_surface_id = self.root_surface_id_for_surface(compositor_surface_id(surface));
        let Some(press) = self.valid_pointer_press_for_surface(root_surface_id, surface, serial)
        else {
            log_begin_rejection_without_target(
                "invalid_press_serial",
                0.0,
                0.0,
                WindowInteractionKind::Move,
                WindowInteractionSource::XdgToplevelMove,
                0,
            );
            return false;
        };
        self.begin_window_interaction_for_root(BeginWindowInteraction {
            window_id: self.window_id_for_surface(root_surface_id),
            root_surface_id,
            x: press.output_x,
            y: press.output_y,
            kind: WindowInteractionKind::Move,
            source: WindowInteractionSource::XdgToplevelMove,
            trigger_button: Some(press.button),
            trigger_serial: Some(press.serial),
            pointer_motion_surface_id: Some(compositor_surface_id(&press.surface)),
        })
    }

    pub(in crate::compositor) fn begin_client_window_resize(
        &mut self,
        surface: &wl_surface::WlSurface,
        serial: u32,
        edges: ResizeEdges,
    ) -> bool {
        let root_surface_id = self.root_surface_id_for_surface(compositor_surface_id(surface));
        let Some(press) = self.valid_pointer_press_for_surface(root_surface_id, surface, serial)
        else {
            log_begin_rejection_without_target(
                "invalid_press_serial",
                0.0,
                0.0,
                WindowInteractionKind::Resize(edges),
                WindowInteractionSource::XdgToplevelResize,
                0,
            );
            return false;
        };
        self.begin_window_interaction_for_root(BeginWindowInteraction {
            window_id: self.window_id_for_surface(root_surface_id),
            root_surface_id,
            x: press.output_x,
            y: press.output_y,
            kind: WindowInteractionKind::Resize(edges),
            source: WindowInteractionSource::XdgToplevelResize,
            trigger_button: Some(press.button),
            trigger_serial: Some(press.serial),
            pointer_motion_surface_id: Some(compositor_surface_id(&press.surface)),
        })
    }

    pub(in crate::compositor) fn begin_x11_client_window_interaction(
        &mut self,
        handle: X11WindowHandle,
        x: f64,
        y: f64,
        kind: WindowInteractionKind,
        x11_button: u32,
    ) -> bool {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return false;
        };
        let Some(root_surface_id) = self.window(window_id).map(|window| window.root_surface_id)
        else {
            return false;
        };
        let requested_button = x11_button_to_evdev(x11_button);
        let Some(press) = self.held_pointer_buttons.iter().rev().find(|press| {
            press.root_surface_id == root_surface_id
                && requested_button.is_none_or(|button| press.button == button)
        }) else {
            return false;
        };
        let trigger_button = press.button;
        let pointer_motion_surface_id = compositor_surface_id(&press.surface);
        self.begin_window_interaction_for_root(BeginWindowInteraction {
            window_id: Some(window_id),
            root_surface_id,
            x,
            y,
            kind,
            source: WindowInteractionSource::X11NetWmMoveResize,
            trigger_button: Some(trigger_button),
            trigger_serial: None,
            pointer_motion_surface_id: Some(pointer_motion_surface_id),
        })
    }

    pub(in crate::compositor) fn cancel_x11_client_window_interaction(
        &mut self,
        handle: X11WindowHandle,
    ) -> bool {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return false;
        };
        let Some(interaction) = self.window_interaction else {
            return false;
        };
        if interaction.window_id != window_id
            || interaction.source != WindowInteractionSource::X11NetWmMoveResize
        {
            return false;
        }
        self.end_window_interaction_by_id_with_reason(
            interaction.id,
            WindowInteractionEndReason::ExplicitCancel,
        )
    }

    pub(in crate::compositor) fn begin_window_interaction_for_root(
        &mut self,
        begin: BeginWindowInteraction,
    ) -> bool {
        if self.window_interaction.is_some() {
            log_begin_rejection(self, begin, "interaction_already_active");
            return false;
        }
        if self.window_interaction_blocked_by_pointer_lock() {
            log_begin_rejection(self, begin, "pointer_lock_active_or_pending");
            return false;
        }
        let BeginWindowInteraction {
            window_id: begin_window_id,
            root_surface_id,
            x,
            y,
            kind,
            source,
            trigger_button,
            trigger_serial,
            pointer_motion_surface_id,
        } = begin;
        let Some(root_surface) = self
            .renderable_surfaces
            .iter()
            .find(|surface| surface.surface_id == root_surface_id)
        else {
            log_begin_rejection(self, begin, "root_missing");
            return false;
        };
        let Some(window_id) =
            begin_window_id.or_else(|| self.window_id_for_surface(root_surface_id))
        else {
            log_begin_rejection(self, begin, "window_identity_missing");
            return false;
        };
        if self
            .window(window_id)
            .is_some_and(|window| !window.is_normal_x11_role())
        {
            return false;
        }
        if let Some(pointer_motion_surface_id) = pointer_motion_surface_id {
            if self
                .surface_resource_by_id(pointer_motion_surface_id)
                .is_none()
            {
                log_begin_rejection(self, begin, "motion_target_missing");
                return false;
            }
            if self.root_surface_id_for_surface(pointer_motion_surface_id) != root_surface_id {
                log_begin_rejection(self, begin, "motion_target_wrong_root");
                return false;
            }
        }
        let fallback_geometry = WindowGeometry::new(
            root_surface.placement,
            root_surface.width,
            root_surface.height,
        );
        let start_geometry = match kind {
            WindowInteractionKind::Resize(_) => self
                .current_visual_root_window_geometry(root_surface_id)
                .unwrap_or(fallback_geometry),
            WindowInteractionKind::Move => self
                .current_root_window_geometry(root_surface_id)
                .unwrap_or(fallback_geometry),
        };
        let Some(root_resource) = self.surface_resource_by_id(root_surface_id) else {
            log_begin_rejection(self, begin, "root_resource_missing");
            return false;
        };
        let resize_interaction_id = match kind {
            WindowInteractionKind::Resize(_) => {
                let id = self.allocate_resize_interaction_id();
                if let Some(flow) = self.resize_configure_flows.get_mut(&root_surface_id) {
                    let result = flow.begin_interaction(id);
                    self.resize_flow_metrics.obsolete_queued_targets_discarded = self
                        .resize_flow_metrics
                        .obsolete_queued_targets_discarded
                        .saturating_add(result.obsolete_queued_discarded as u64);
                    self.resize_flow_metrics.obsolete_finals_discarded = self
                        .resize_flow_metrics
                        .obsolete_finals_discarded
                        .saturating_add(result.obsolete_final_discarded as u64);
                    self.resize_flow_metrics
                        .obsolete_in_flight_configures_discarded = self
                        .resize_flow_metrics
                        .obsolete_in_flight_configures_discarded
                        .saturating_add(
                            u64::try_from(result.obsolete_in_flight_discarded).unwrap_or(u64::MAX),
                        );
                    if result.obsolete_in_flight_discarded > 0
                        && compositor_debug_surface_logging_enabled()
                    {
                        eprintln!(
                            "oblivion-one compositor: resize_flow surface={root_surface_id} decision=superseded interaction_id={} obsolete_in_flight={}",
                            id.get(),
                            result.obsolete_in_flight_discarded,
                        );
                    }
                }
                self.resize_flow_metrics.resize_interactions_started = self
                    .resize_flow_metrics
                    .resize_interactions_started
                    .saturating_add(1);
                if self.active_toplevel_resizes.contains_key(&root_surface_id) {
                    self.resize_flow_metrics.rapid_reresize_interactions = self
                        .resize_flow_metrics
                        .rapid_reresize_interactions
                        .saturating_add(1);
                }
                Some(id)
            }
            WindowInteractionKind::Move => None,
        };
        if matches!(kind, WindowInteractionKind::Resize(_)) {
            self.resize_flow_metrics.visual_geometry_resize_starts = self
                .resize_flow_metrics
                .visual_geometry_resize_starts
                .saturating_add(1);
        }
        let start_width = start_geometry.width;
        let start_height = start_geometry.height;
        let start_placement = start_geometry.placement;

        self.focus_surface(root_resource);
        let id = self.allocate_window_interaction_id();
        self.window_interaction = Some(WindowInteraction {
            id,
            window_id,
            root_surface_id,
            kind,
            source,
            trigger_button,
            trigger_serial,
            pointer_motion_surface_id,
            start_pointer_x: x,
            start_pointer_y: y,
            start_placement,
            start_width,
            start_height,
            drag_committed: false,
            resize_interaction_id,
        });
        self.set_interaction_cursor_override(kind);
        let snapshot = self
            .window_interaction
            .expect("window interaction was just installed")
            .debug_snapshot();
        resize_debug_log(|| {
            format!(
                "event=begin interaction_id={} resize_interaction_id={} root={} motion_target={} source={:?} kind={:?} trigger_button={} trigger_serial={} pointer=({},{}) start_geometry=({},{},{},{},{:?}) cursor_override={:?} pointer_lock_blocked=false",
                snapshot.interaction_id,
                snapshot
                    .resize_interaction_id
                    .map_or_else(|| "none".to_string(), |id| id.to_string()),
                snapshot.root_surface_id,
                snapshot
                    .pointer_motion_surface_id
                    .map_or_else(|| "none".to_string(), |id| id.to_string()),
                snapshot.source,
                snapshot.kind,
                snapshot
                    .trigger_button
                    .map_or_else(|| "none".to_string(), |button| button.to_string()),
                snapshot
                    .trigger_serial
                    .map_or_else(|| "none".to_string(), |serial| serial.to_string()),
                snapshot.start_pointer_x,
                snapshot.start_pointer_y,
                start_width,
                start_height,
                start_placement.local_x,
                start_placement.local_y,
                start_placement.root_mode,
                self.interaction_cursor_override,
            )
        });
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: window_interaction begin id={} root={} source={:?} kind={:?} trigger_button={:?} trigger_serial={:?}",
                id.get(),
                root_surface_id,
                source,
                kind,
                trigger_button,
                trigger_serial,
            );
        }
        true
    }

    fn set_interaction_cursor_override(&mut self, kind: WindowInteractionKind) {
        let next = Some(InteractionCursorOverride {
            shape: InteractionCursorShape::for_window_interaction(kind),
        });
        if self.interaction_cursor_override == next {
            return;
        }
        self.interaction_cursor_override = next;
        self.advance_render_generation(RenderGenerationCause::CursorState);
        self.sync_cursor_visibility_request();
    }

    pub(in crate::compositor) fn clear_window_interaction_state(
        &mut self,
        reason: WindowInteractionEndReason,
    ) -> bool {
        let interaction = self.window_interaction;
        let snapshot = interaction.map(WindowInteraction::debug_snapshot);
        let root_surface_id = snapshot.map(|snapshot| snapshot.root_surface_id);
        let pending_resize = root_surface_id.and_then(|surface_id| {
            self.resize_configure_flows.get(&surface_id).map(|flow| {
                (
                    flow.outstanding_count(),
                    flow.acked_uncaptured_count(),
                    flow.captured_count(),
                    flow.queued_latest(),
                    flow.final_pending(),
                )
            })
        });
        let visual_geometry = root_surface_id
            .and_then(|surface_id| self.current_visual_root_window_geometry(surface_id));
        let committed_geometry =
            root_surface_id.and_then(|surface_id| self.current_root_window_geometry(surface_id));
        resize_debug_log(|| {
            format!(
                "event=end reason={reason:?} interaction_id={} resize_interaction_id={} root={} kind={:?} source={:?} trigger_button={} trigger_serial={} drag_committed={} pending_resize={pending_resize:?} visual_geometry={visual_geometry:?} committed_geometry={committed_geometry:?}",
                snapshot.map_or_else(
                    || "none".to_string(),
                    |snapshot| snapshot.interaction_id.to_string()
                ),
                snapshot
                    .and_then(|snapshot| snapshot.resize_interaction_id)
                    .map_or_else(|| "none".to_string(), |id| id.to_string()),
                snapshot.map_or_else(
                    || "none".to_string(),
                    |snapshot| snapshot.root_surface_id.to_string()
                ),
                snapshot.map(|snapshot| snapshot.kind),
                snapshot.map(|snapshot| snapshot.source),
                snapshot
                    .and_then(|snapshot| snapshot.trigger_button)
                    .map_or_else(|| "none".to_string(), |button| button.to_string()),
                snapshot
                    .and_then(|snapshot| snapshot.trigger_serial)
                    .map_or_else(|| "none".to_string(), |serial| serial.to_string()),
                snapshot.is_some_and(|snapshot| snapshot.drag_committed),
            )
        });
        let had_interaction = self.window_interaction.take().is_some();
        let had_cursor_override = self.interaction_cursor_override.take().is_some();
        if had_cursor_override {
            self.advance_render_generation(RenderGenerationCause::CursorState);
            self.sync_cursor_visibility_request();
            self.resume_pending_pointer_constraint_activation();
        }
        had_interaction || had_cursor_override
    }

    pub(in crate::compositor) fn interaction_cursor_override_active(&self) -> bool {
        self.interaction_cursor_override.is_some()
    }

    pub(in crate::compositor) fn allocate_window_interaction_id(&mut self) -> WindowInteractionId {
        self.next_window_interaction_id = self.next_window_interaction_id.saturating_add(1);
        WindowInteractionId::new(self.next_window_interaction_id.max(1))
    }

    pub(in crate::compositor) fn allocate_resize_interaction_id(&mut self) -> ResizeInteractionId {
        self.next_resize_interaction_id = self.next_resize_interaction_id.saturating_add(1);
        ResizeInteractionId::new(self.next_resize_interaction_id.max(1))
    }

    pub(in crate::compositor) fn valid_pointer_press_for_surface(
        &self,
        root_surface_id: u32,
        surface: &wl_surface::WlSurface,
        serial: u32,
    ) -> Option<PointerPress> {
        let press = self.last_pointer_press.as_ref()?;
        let valid_surface = press.root_surface_id == root_surface_id
            || press.surface.id().same_client_as(&surface.id());
        (press.serial == serial && valid_surface).then_some(press.clone())
    }

    pub(in crate::compositor) fn window_frame_hit_at(
        &mut self,
        x: f64,
        y: f64,
    ) -> Option<WindowFrameHit> {
        if let Some(hit) = self.root_surface_hit_at(x, y) {
            if self
                .window(hit.window_id)
                .is_some_and(|window| !window.is_normal_x11_role())
            {
                return None;
            }
            let kind = window_frame_action_for_local_point(
                hit.local_x,
                hit.local_y,
                hit.width,
                hit.height,
            )?;
            return Some(WindowFrameHit {
                window_id: hit.window_id,
                root_surface_id: hit.root_surface_id,
                kind,
            });
        }

        None
    }

    pub(in crate::compositor) fn update_window_interaction(&mut self, x: f64, y: f64) -> bool {
        let Some(interaction_id) = self.active_window_interaction_id() else {
            return false;
        };
        self.update_window_interaction_by_id(interaction_id, x, y)
    }

    pub(in crate::compositor) fn update_window_interaction_by_id(
        &mut self,
        interaction_id: WindowInteractionId,
        x: f64,
        y: f64,
    ) -> bool {
        let Some(mut interaction) = self.window_interaction else {
            return false;
        };
        if interaction.id != interaction_id {
            return false;
        }
        let dx = (x - interaction.start_pointer_x).round() as i32;
        let dy = (y - interaction.start_pointer_y).round() as i32;

        match interaction.kind {
            WindowInteractionKind::Move => {
                let placement = SurfacePlacement {
                    local_x: interaction.start_placement.local_x + dx,
                    local_y: interaction.start_placement.local_y + dy,
                    ..interaction.start_placement
                };
                let moved = self.set_surface_placement_with_cause(
                    interaction.root_surface_id,
                    placement,
                    RenderGenerationCause::WindowMove,
                );
                if moved {
                    self.queue_backend_configure(
                        interaction.window_id,
                        WindowGeometry::new(
                            placement,
                            interaction.start_width,
                            interaction.start_height,
                        ),
                        self.window(interaction.window_id)
                            .map(|window| window.state.mode())
                            .unwrap_or(ToplevelMode::Floating),
                        false,
                    );
                }
                moved
            }
            WindowInteractionKind::Resize(edges) => {
                if !interaction.drag_committed && !resize_drag_threshold_reached(edges, dx, dy) {
                    return false;
                }
                interaction.drag_committed = true;
                self.window_interaction = Some(interaction);

                let resize = interactive_resize_geometry(interaction, edges, dx, dy);
                let update = PendingInteractiveResizeUpdate {
                    root_surface_id: interaction.root_surface_id,
                    width: resize.width,
                    height: resize.height,
                    placement: SurfacePlacement {
                        local_x: resize.x,
                        local_y: resize.y,
                        ..interaction.start_placement
                    },
                    edges,
                    interaction_id: interaction
                        .resize_interaction_id
                        .expect("resize interaction has an ID"),
                };
                self.resize_flow_metrics.raw_pointer_resize_updates = self
                    .resize_flow_metrics
                    .raw_pointer_resize_updates
                    .saturating_add(1);
                if self.pending_interactive_resize_update == Some(update) {
                    self.resize_flow_metrics.resize_updates_skipped_unchanged = self
                        .resize_flow_metrics
                        .resize_updates_skipped_unchanged
                        .saturating_add(1);
                    return false;
                }
                let pending_update_replaced = self
                    .pending_interactive_resize_update
                    .replace(update)
                    .is_some();
                if pending_update_replaced {
                    self.resize_flow_metrics.pending_resize_updates_replaced = self
                        .resize_flow_metrics
                        .pending_resize_updates_replaced
                        .saturating_add(1);
                }
                resize_debug_log(|| {
                    format!(
                        "event=update interaction_id={} root={} pointer=({x},{y}) delta=({dx},{dy}) drag_committed={} pending_update_replaced={} target_geometry=({},{},{},{},{:?})",
                        interaction.id.get(),
                        interaction.root_surface_id,
                        interaction.drag_committed,
                        pending_update_replaced,
                        update.placement.local_x,
                        update.placement.local_y,
                        update.width,
                        update.height,
                        update.placement.root_mode,
                    )
                });
                true
            }
        }
    }

    pub(in crate::compositor) fn end_window_interaction(&mut self) {
        let Some(interaction_id) = self.active_window_interaction_id() else {
            return;
        };
        self.end_window_interaction_by_id_with_reason(
            interaction_id,
            WindowInteractionEndReason::ExplicitEnd,
        );
    }

    pub(in crate::compositor) fn cancel_window_interaction(
        &mut self,
        reason: WindowInteractionEndReason,
    ) -> bool {
        if self.window_interaction.is_none() && self.interaction_cursor_override.is_none() {
            return false;
        }
        self.clear_window_interaction_state(reason)
    }

    pub(in crate::compositor) fn end_window_interaction_by_id_with_reason(
        &mut self,
        interaction_id: WindowInteractionId,
        reason: WindowInteractionEndReason,
    ) -> bool {
        let interaction = self.window_interaction;
        if interaction.is_none_or(|interaction| interaction.id != interaction_id) {
            return false;
        }
        if let Some(interaction) = interaction
            && interaction.drag_committed
            && let WindowInteractionKind::Resize(edges) = interaction.kind
        {
            self.apply_pending_interactive_resize_update();
            self.send_resize_end_configure(
                interaction.root_surface_id,
                edges,
                interaction
                    .resize_interaction_id
                    .expect("resize interaction has an ID"),
            );
        }
        if let Some(interaction) = interaction
            && compositor_debug_surface_logging_enabled()
        {
            eprintln!(
                "oblivion-one compositor: window_interaction end id={} root={} source={:?} kind={:?} trigger_button={:?} trigger_serial={:?}",
                interaction.id.get(),
                interaction.root_surface_id,
                interaction.source,
                interaction.kind,
                interaction.trigger_button,
                interaction.trigger_serial,
            );
        }
        self.clear_window_interaction_state(reason);
        true
    }

    pub(in crate::compositor) fn end_window_interaction_for_button(&mut self, button: u32) -> bool {
        let Some(interaction) = self.window_interaction else {
            return false;
        };
        if interaction.trigger_button != Some(button) {
            return false;
        }
        self.end_window_interaction_by_id_with_reason(
            interaction.id,
            WindowInteractionEndReason::TriggerButtonRelease,
        )
    }

    pub(in crate::compositor) fn apply_pending_interactive_resize_update(&mut self) -> bool {
        let Some(update) = self.pending_interactive_resize_update.take() else {
            return false;
        };
        let applied = self.queue_resize_root_window_to(
            update.root_surface_id,
            update.width,
            update.height,
            update.placement,
            update.edges,
            update.interaction_id,
        );
        if applied {
            self.resize_flow_metrics.resize_updates_applied = self
                .resize_flow_metrics
                .resize_updates_applied
                .saturating_add(1);
        } else {
            self.resize_flow_metrics.resize_updates_skipped_unchanged = self
                .resize_flow_metrics
                .resize_updates_skipped_unchanged
                .saturating_add(1);
        }
        applied
    }

    pub(in crate::compositor) fn window_interaction_active(&self) -> bool {
        self.window_interaction.is_some()
    }

    pub(in crate::compositor) fn active_window_interaction_id(
        &self,
    ) -> Option<WindowInteractionId> {
        self.window_interaction.map(|interaction| interaction.id)
    }

    pub(in crate::compositor) fn active_window_interaction_trigger_button(&self) -> Option<u32> {
        self.window_interaction
            .and_then(|interaction| interaction.trigger_button)
    }

    pub(in crate::compositor) fn reconcile_window_interaction_trigger(
        &mut self,
        trigger_pressed: bool,
    ) -> bool {
        let Some(interaction) = self.window_interaction else {
            return false;
        };
        if interaction.trigger_button.is_none() || trigger_pressed {
            return false;
        }
        resize_debug_log(|| {
            format!(
                "event=invariant_violation type=trigger_not_held interaction_id={} trigger_button={}",
                interaction.id.get(),
                interaction.trigger_button.unwrap_or_default(),
            )
        });
        self.end_window_interaction_by_id_with_reason(
            interaction.id,
            WindowInteractionEndReason::TriggerButtonNoLongerHeld,
        )
    }

    pub(in crate::compositor) fn window_interaction_debug_snapshot(
        &self,
    ) -> Option<WindowInteractionDebugSnapshot> {
        self.window_interaction
            .map(WindowInteraction::debug_snapshot)
    }
}

const fn x11_button_to_evdev(button: u32) -> Option<u32> {
    match button {
        0 => None,
        1 => Some(0x110),
        2 => Some(0x112),
        3 => Some(0x111),
        _ => Some(u32::MAX),
    }
}

fn log_begin_rejection(state: &CompositorState, begin: BeginWindowInteraction, reason: &str) {
    let active = state.window_interaction_debug_snapshot();
    resize_debug_log(|| format_begin_rejection(reason, begin, active));
}

pub(in crate::compositor) fn format_begin_rejection(
    reason: &str,
    begin: BeginWindowInteraction,
    active: Option<WindowInteractionDebugSnapshot>,
) -> String {
    let active_fields = active.map_or_else(
        || {
            "active_interaction_id=none active_root=none active_kind=none active_trigger_button=none active_drag_committed=false".to_string()
        },
        |active| {
            format!(
                "active_interaction_id={} active_root={} active_kind={:?} active_trigger_button={} active_drag_committed={}",
                active.interaction_id,
                active.root_surface_id,
                active.kind,
                active
                    .trigger_button
                    .map_or_else(|| "none".to_string(), |button| button.to_string()),
                active.drag_committed,
            )
        },
    );
    format!(
        "event=begin reason={reason} root={} motion_target={} source={:?} kind={:?} trigger_button={} trigger_serial={} pointer=({},{}) {active_fields}",
        begin.root_surface_id,
        begin
            .pointer_motion_surface_id
            .map_or_else(|| "none".to_string(), |id| id.to_string()),
        begin.source,
        begin.kind,
        begin
            .trigger_button
            .map_or_else(|| "none".to_string(), |button| button.to_string()),
        begin
            .trigger_serial
            .map_or_else(|| "none".to_string(), |serial| serial.to_string()),
        begin.x,
        begin.y,
    )
}

fn log_begin_rejection_without_target(
    reason: &str,
    x: f64,
    y: f64,
    kind: WindowInteractionKind,
    source: WindowInteractionSource,
    trigger_button: u32,
) {
    resize_debug_log(|| {
        format!(
            "event=begin reason={reason} root=none motion_target=none source={source:?} kind={kind:?} trigger_button={} trigger_serial=none pointer=({x},{y}) active_interaction_id=none active_root=none active_kind=none active_trigger_button=none active_drag_committed=false",
            if trigger_button == 0 {
                "none".to_string()
            } else {
                trigger_button.to_string()
            },
        )
    });
}
