use super::*;

#[derive(Debug, Clone, Copy)]
pub(in crate::compositor) struct BeginWindowInteraction {
    root_surface_id: u32,
    x: f64,
    y: f64,
    kind: WindowInteractionKind,
    source: WindowInteractionSource,
    trigger_button: Option<u32>,
    trigger_serial: Option<u32>,
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
            return false;
        };
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        let Some((local_x, local_y, width, height)) =
            self.root_window_local_point_at(root_surface_id, x, y)
        else {
            return self.begin_window_interaction_for_root(BeginWindowInteraction {
                root_surface_id,
                x,
                y,
                kind: WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT),
                source: WindowInteractionSource::NativeBinding,
                trigger_button: (trigger_button != 0).then_some(trigger_button),
                trigger_serial: None,
            });
        };
        let edges = resize_edges_for_window_point(local_x, local_y, width, height);
        self.begin_window_interaction_for_root(BeginWindowInteraction {
            root_surface_id,
            x,
            y,
            kind: WindowInteractionKind::Resize(edges),
            source: WindowInteractionSource::NativeBinding,
            trigger_button: (trigger_button != 0).then_some(trigger_button),
            trigger_serial: None,
        })
    }

    pub(in crate::compositor) fn begin_window_frame_action_at(&mut self, x: f64, y: f64) -> bool {
        let Some(hit) = self.window_frame_hit_at(x, y) else {
            return false;
        };
        self.begin_window_interaction_for_root(BeginWindowInteraction {
            root_surface_id: hit.root_surface_id,
            x,
            y,
            kind: hit.kind,
            source: WindowInteractionSource::NativeBinding,
            trigger_button: None,
            trigger_serial: None,
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
            return false;
        };
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        self.begin_window_interaction_for_root(BeginWindowInteraction {
            root_surface_id,
            x,
            y,
            kind,
            source,
            trigger_button,
            trigger_serial,
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
            return false;
        };
        self.begin_window_interaction_for_root(BeginWindowInteraction {
            root_surface_id,
            x: press.output_x,
            y: press.output_y,
            kind: WindowInteractionKind::Move,
            source: WindowInteractionSource::XdgToplevelMove,
            trigger_button: Some(press.button),
            trigger_serial: Some(press.serial),
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
            return false;
        };
        self.begin_window_interaction_for_root(BeginWindowInteraction {
            root_surface_id,
            x: press.output_x,
            y: press.output_y,
            kind: WindowInteractionKind::Resize(edges),
            source: WindowInteractionSource::XdgToplevelResize,
            trigger_button: Some(press.button),
            trigger_serial: Some(press.serial),
        })
    }

    pub(in crate::compositor) fn begin_window_interaction_for_root(
        &mut self,
        begin: BeginWindowInteraction,
    ) -> bool {
        if self.window_interaction.is_some() {
            return false;
        }
        let BeginWindowInteraction {
            root_surface_id,
            x,
            y,
            kind,
            source,
            trigger_button,
            trigger_serial,
        } = begin;
        let Some(root_surface) = self
            .renderable_surfaces
            .iter()
            .find(|surface| surface.surface_id == root_surface_id)
        else {
            return false;
        };
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
            root_surface_id,
            kind,
            source,
            trigger_button,
            trigger_serial,
            start_pointer_x: x,
            start_pointer_y: y,
            start_placement,
            start_width,
            start_height,
            drag_committed: false,
            resize_interaction_id,
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
            let kind = window_frame_action_for_local_point(
                hit.local_x,
                hit.local_y,
                hit.width,
                hit.height,
            )?;
            return Some(WindowFrameHit {
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
                let placement = SurfacePlacement::root_at(
                    interaction.start_placement.local_x + dx,
                    interaction.start_placement.local_y + dy,
                );
                self.set_surface_placement_with_cause(
                    interaction.root_surface_id,
                    placement,
                    RenderGenerationCause::WindowMove,
                )
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
                    placement: SurfacePlacement::root_at(resize.x, resize.y),
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
                if self
                    .pending_interactive_resize_update
                    .replace(update)
                    .is_some()
                {
                    self.resize_flow_metrics.pending_resize_updates_replaced = self
                        .resize_flow_metrics
                        .pending_resize_updates_replaced
                        .saturating_add(1);
                }
                true
            }
        }
    }

    pub(in crate::compositor) fn end_window_interaction(&mut self) {
        let Some(interaction_id) = self.active_window_interaction_id() else {
            return;
        };
        self.end_window_interaction_by_id(interaction_id);
    }

    pub(in crate::compositor) fn end_window_interaction_by_id(
        &mut self,
        interaction_id: WindowInteractionId,
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
        self.window_interaction = None;
        true
    }

    pub(in crate::compositor) fn end_window_interaction_for_button(&mut self, button: u32) -> bool {
        let Some(interaction) = self.window_interaction else {
            return false;
        };
        if interaction.trigger_button != Some(button) {
            return false;
        }
        self.end_window_interaction_by_id(interaction.id)
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
}
