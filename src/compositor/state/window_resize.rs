use super::*;

impl CompositorState {
    pub(in crate::compositor) fn queue_resize_root_window_to(
        &mut self,
        surface_id: u32,
        width: u32,
        height: u32,
        placement: SurfacePlacement,
        edges: ResizeEdges,
        interaction_id: ResizeInteractionId,
    ) -> bool {
        if !self.toplevel_surfaces.contains_key(&surface_id) {
            return false;
        };
        let geometry = self.clamp_resize_geometry(
            surface_id,
            WindowGeometry::new(placement, width, height),
            edges,
        );
        let width = geometry.width;
        let height = geometry.height;
        let placement = geometry.placement;
        let pending = PendingResizeConfigure {
            surface_id,
            width,
            height,
            placement,
            edges,
            resizing: true,
            interaction_id,
        };
        self.resize_flow_metrics.configures_requested = self
            .resize_flow_metrics
            .configures_requested
            .saturating_add(1);
        let flow = self.resize_configure_flows.entry(surface_id).or_default();
        let was_blocked = flow.has_in_flight() || flow.latest_desired().is_some();
        let queued = flow.queue(pending);
        self.update_resize_retained_configure_peak(surface_id);
        if !queued {
            self.resize_flow_metrics.duplicate_configure_sizes_skipped = self
                .resize_flow_metrics
                .duplicate_configure_sizes_skipped
                .saturating_add(1);
        }
        if queued && was_blocked {
            self.resize_flow_metrics.geometries_coalesced = self
                .resize_flow_metrics
                .geometries_coalesced
                .saturating_add(1);
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: resize_flow surface={surface_id} decision=coalesced queued_serial=not-sent queued_size={}x{} final_pending=false preview_active=true",
                    pending.width, pending.height,
                );
            }
        }
        self.preview_resize_root_window_to(
            surface_id,
            width,
            height,
            placement,
            edges,
            interaction_id,
        )
    }

    pub(in crate::compositor) fn clamp_resize_geometry(
        &self,
        surface_id: u32,
        geometry: WindowGeometry,
        edges: ResizeEdges,
    ) -> WindowGeometry {
        let width = self.clamp_toplevel_width(surface_id, geometry.width);
        let height = self.clamp_toplevel_height(surface_id, geometry.height);
        let mut placement = geometry.placement;
        if edges.left && width != geometry.width {
            let requested_right = placement
                .local_x
                .saturating_add(i32::try_from(geometry.width).unwrap_or(i32::MAX));
            placement.local_x =
                requested_right.saturating_sub(i32::try_from(width).unwrap_or(i32::MAX));
        }
        if edges.top && height != geometry.height {
            let requested_bottom = placement
                .local_y
                .saturating_add(i32::try_from(geometry.height).unwrap_or(i32::MAX));
            placement.local_y =
                requested_bottom.saturating_sub(i32::try_from(height).unwrap_or(i32::MAX));
        }

        WindowGeometry::new(placement, width, height)
    }

    pub(in crate::compositor) fn clamp_toplevel_width(&self, surface_id: u32, width: u32) -> u32 {
        let constraints = self.toplevel_constraints(surface_id);
        let min_width = constraints.min_width.unwrap_or(MIN_WINDOW_WIDTH);
        let mut clamped = width.max(min_width);
        if let Some(max_width) = constraints.max_width {
            clamped = clamped.min(max_width.max(min_width));
        }
        clamped
    }

    pub(in crate::compositor) fn clamp_toplevel_height(&self, surface_id: u32, height: u32) -> u32 {
        let constraints = self.toplevel_constraints(surface_id);
        let min_height = constraints.min_height.unwrap_or(MIN_WINDOW_HEIGHT);
        let mut clamped = height.max(min_height);
        if let Some(max_height) = constraints.max_height {
            clamped = clamped.min(max_height.max(min_height));
        }
        clamped
    }

    pub(in crate::compositor) fn toplevel_constraints(
        &self,
        surface_id: u32,
    ) -> ToplevelSizeConstraints {
        self.toplevel_surfaces
            .get(&surface_id)
            .map(|toplevel| toplevel.constraints)
            .unwrap_or_default()
    }

    pub(in crate::compositor) fn preview_resize_root_window_to(
        &mut self,
        surface_id: u32,
        width: u32,
        height: u32,
        placement: SurfacePlacement,
        edges: ResizeEdges,
        interaction_id: ResizeInteractionId,
    ) -> bool {
        let flow_sequence = self
            .resize_configure_flows
            .get(&surface_id)
            .and_then(ResizeConfigureFlow::in_flight_sequence)
            .unwrap_or_else(|| self.next_resize_configure_sequence.saturating_add(1));
        let previous = self
            .toplevel_visual_geometries
            .get(&surface_id)
            .copied()
            .or_else(|| {
                self.current_visual_root_window_geometry(surface_id)
                    .map(|geometry| ToplevelVisualGeometry {
                        placement: geometry.placement,
                        width: geometry.width,
                        height: geometry.height,
                        active_resize: None,
                    })
            });
        if previous.is_some_and(|visual| {
            visual.width == width
                && visual.height == height
                && visual.placement == placement
                && visual.active_resize == Some(interaction_id)
        }) {
            return false;
        }

        self.toplevel_visual_geometries.insert(
            surface_id,
            ToplevelVisualGeometry {
                placement,
                width,
                height,
                active_resize: Some(interaction_id),
            },
        );
        self.update_toplevel_visual_render_assignment(surface_id);
        let previous_resize = self.active_toplevel_resizes.get(&surface_id).copied();
        if previous_resize.is_none() {
            self.active_toplevel_resizes.insert(
                surface_id,
                ActiveToplevelResize {
                    interaction_id,
                    flow_sequence,
                    edges,
                    activated_at: Instant::now(),
                },
            );
            self.resize_flow_metrics.preview_activations = self
                .resize_flow_metrics
                .preview_activations
                .saturating_add(1);
        } else if previous_resize.is_some_and(|resize| resize.interaction_id != interaction_id) {
            self.active_toplevel_resizes.insert(
                surface_id,
                ActiveToplevelResize {
                    interaction_id,
                    flow_sequence,
                    edges,
                    activated_at: Instant::now(),
                },
            );
            self.resize_flow_metrics.preview_ownership_transfers = self
                .resize_flow_metrics
                .preview_ownership_transfers
                .saturating_add(1);
        }
        self.advance_render_generation(RenderGenerationCause::WindowResize);
        true
    }

    pub(in crate::compositor) fn update_toplevel_visual_render_assignment(
        &mut self,
        root_surface_id: u32,
    ) {
        let geometry = self
            .surface_window_geometries
            .get(&root_surface_id)
            .copied();
        let geometry_x = geometry.map_or(0, |geometry| geometry.x);
        let geometry_y = geometry.map_or(0, |geometry| geometry.y);
        let authoritative = self.surface_placement(root_surface_id);
        let visual = self
            .toplevel_visual_geometries
            .get(&root_surface_id)
            .copied()
            .map(|visual| {
                (
                    visual.placement,
                    visual.width,
                    visual.height,
                    visual.active_resize,
                )
            })
            .or_else(|| {
                (authoritative.root_mode == RootPlacementMode::Absolute).then(|| {
                    let surface = self
                        .renderable_surfaces
                        .iter()
                        .find(|surface| surface.surface_id == root_surface_id);
                    let (width, height) = self
                        .xdg_window_geometry_size(root_surface_id)
                        .or_else(|| surface.map(|surface| (surface.width, surface.height)))
                        .unwrap_or_default();
                    (authoritative, width, height, None)
                })
            });
        let Some((visual_placement, visual_width, visual_height, active_resize)) = visual else {
            let placements = &self.surface_placements;
            for surface in &mut self.renderable_surfaces {
                if root_surface_id_for_surface_in_placements(placements, surface.surface_id)
                    == root_surface_id
                {
                    surface.render_placement = None;
                    surface.visual_clip = None;
                }
            }
            self.invalidate_surface_origin_cache();
            return;
        };
        if visual_width == 0 || visual_height == 0 {
            return;
        }
        let root_render_placement = SurfacePlacement {
            parent_surface_id: None,
            local_x: visual_placement.local_x.saturating_sub(geometry_x),
            local_y: visual_placement.local_y.saturating_sub(geometry_y),
            root_mode: visual_placement.root_mode,
        };
        let clip = render::SurfaceTargetRect::new(
            visual_placement.local_x,
            visual_placement.local_y,
            visual_width,
            visual_height,
        );
        let visual_clip = active_resize.is_some().then_some(clip);
        let placements = &self.surface_placements;
        for surface in &mut self.renderable_surfaces {
            if root_surface_id_for_surface_in_placements(placements, surface.surface_id)
                != root_surface_id
            {
                continue;
            }
            surface.visual_clip = visual_clip;
            if surface.surface_id == root_surface_id {
                surface.render_placement = Some(root_render_placement);
            }
        }
        self.invalidate_surface_origin_cache();
    }

    pub(in crate::compositor) fn clear_toplevel_visual_render_assignment(
        &mut self,
        root_surface_id: u32,
    ) {
        let placements = &self.surface_placements;
        for surface in &mut self.renderable_surfaces {
            if root_surface_id_for_surface_in_placements(placements, surface.surface_id)
                == root_surface_id
            {
                surface.render_placement = None;
                surface.visual_clip = None;
            }
        }
        self.invalidate_surface_origin_cache();
    }

    pub(in crate::compositor) fn flush_pending_resize_configure(&mut self) -> bool {
        let surface_ids = self
            .resize_configure_flows
            .iter()
            .filter_map(|(surface_id, flow)| flow.has_sendable().then_some(*surface_id))
            .collect::<Vec<_>>();
        let mut sent = false;
        for surface_id in surface_ids {
            let desired = self
                .resize_configure_flows
                .get_mut(&surface_id)
                .and_then(ResizeConfigureFlow::take_sendable);
            if let Some(desired) = desired {
                sent |= self.send_resize_configure(desired);
            }
        }
        sent
    }

    pub(in crate::compositor) fn send_resize_end_configure(
        &mut self,
        surface_id: u32,
        edges: ResizeEdges,
        interaction_id: ResizeInteractionId,
    ) -> bool {
        let desired = self
            .resize_configure_flows
            .get(&surface_id)
            .and_then(ResizeConfigureFlow::latest_desired)
            .filter(|pending| pending.interaction_id == interaction_id)
            .map(|pending| PendingResizeConfigure {
                resizing: false,
                ..pending
            })
            .or_else(|| {
                self.current_visual_root_window_geometry(surface_id)
                    .map(|geometry| PendingResizeConfigure {
                        surface_id,
                        width: geometry.width,
                        height: geometry.height,
                        placement: geometry.placement,
                        edges,
                        resizing: false,
                        interaction_id,
                    })
            });
        let Some(desired) = desired else {
            return false;
        };
        self.resize_flow_metrics.configures_requested = self
            .resize_flow_metrics
            .configures_requested
            .saturating_add(1);
        self.resize_configure_flows
            .entry(surface_id)
            .or_default()
            .queue_final(desired);
        self.update_resize_retained_configure_peak(surface_id);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: resize_flow surface={surface_id} decision=coalesced queued_serial=not-sent queued_size={}x{} final_pending=true preview_active={}",
                desired.width,
                desired.height,
                self.active_toplevel_resizes.contains_key(&surface_id),
            );
        }
        self.flush_pending_resize_configure()
    }

    pub(in crate::compositor) fn pending_resize_configure_is_flushable(&self) -> bool {
        self.resize_configure_flows
            .values()
            .any(ResizeConfigureFlow::has_sendable)
    }

    pub(in crate::compositor) fn send_resize_configure(
        &mut self,
        desired: PendingResizeConfigure,
    ) -> bool {
        let surface_id = desired.surface_id;
        let geometry = self.clamp_resize_geometry(
            surface_id,
            WindowGeometry::new(desired.placement, desired.width, desired.height),
            desired.edges,
        );
        let width = geometry.width;
        let height = geometry.height;
        let placement = geometry.placement;
        let resizing_states = [xdg_toplevel::State::Resizing];
        let states = if desired.resizing {
            &resizing_states[..]
        } else {
            &[][..]
        };
        let Some(serial) = self.send_configure_root_window_to(surface_id, width, height, states)
        else {
            return false;
        };
        let resize = PendingResizeConfigure {
            surface_id,
            width: width.max(MIN_WINDOW_WIDTH),
            height: height.max(MIN_WINDOW_HEIGHT),
            placement,
            edges: desired.edges,
            resizing: desired.resizing,
            interaction_id: desired.interaction_id,
        };
        self.next_resize_configure_sequence = self.next_resize_configure_sequence.saturating_add(1);
        let sequence = self.next_resize_configure_sequence;
        self.resize_configure_flows
            .entry(surface_id)
            .or_default()
            .mark_sent(resize, serial, sequence);
        self.update_resize_retained_configure_peak(surface_id);
        if !resize.resizing {
            self.resize_flow_metrics.final_configures_sent = self
                .resize_flow_metrics
                .final_configures_sent
                .saturating_add(1);
        }
        self.resize_flow_metrics.configures_sent =
            self.resize_flow_metrics.configures_sent.saturating_add(1);
        self.resize_flow_metrics.max_in_flight_configures =
            self.resize_flow_metrics.max_in_flight_configures.max(
                self.resize_configure_flows
                    .get(&surface_id)
                    .map_or(0, ResizeConfigureFlow::in_flight_configure_count),
            );
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: resize_flow surface={surface_id} decision=sent serial={serial} sequence={sequence} size={}x{} placement={},{} edges={:?} resizing={} in_flight_serial={serial}",
                resize.width,
                resize.height,
                resize.placement.local_x,
                resize.placement.local_y,
                resize.edges,
                resize.resizing,
            );
        }
        true
    }
}
