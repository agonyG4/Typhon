use super::*;

impl CompositorState {
    pub(in crate::compositor) fn begin_x11_resize_for_test(
        &mut self,
        handle: crate::xwayland::X11WindowHandle,
        geometry: crate::xwayland::xwm::X11Geometry,
    ) -> bool {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return false;
        };
        let Some(root_surface_id) = self.window(window_id).map(|window| window.root_surface_id)
        else {
            return false;
        };
        self.queue_resize_root_window_to(
            root_surface_id,
            geometry.width,
            geometry.height,
            SurfacePlacement::root_at(geometry.x, geometry.y),
            ResizeEdges::BOTTOM_RIGHT,
            ResizeInteractionId::new(0x7fff_ffff),
        )
    }

    pub(in crate::compositor) fn finalize_x11_resize_for_test(
        &mut self,
        handle: crate::xwayland::X11WindowHandle,
        geometry: crate::xwayland::xwm::X11Geometry,
    ) -> bool {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return false;
        };
        let Some(mode) = self.window(window_id).map(|window| window.state.mode()) else {
            return false;
        };
        self.queue_backend_finalize_resize(
            window_id,
            WindowGeometry::new(
                SurfacePlacement::root_at(geometry.x, geometry.y),
                geometry.width,
                geometry.height,
            ),
            mode,
        );
        true
    }

    pub(in crate::compositor) fn queue_resize_root_window_to(
        &mut self,
        surface_id: u32,
        width: u32,
        height: u32,
        placement: SurfacePlacement,
        edges: ResizeEdges,
        interaction_id: ResizeInteractionId,
    ) -> bool {
        let Some(window_id) = self.window_id_for_surface(surface_id) else {
            return false;
        };
        if !self.toplevel_surfaces.contains_key(&surface_id) {
            let geometry = self.clamp_resize_geometry(
                surface_id,
                WindowGeometry::new(placement, width, height),
                edges,
            );
            let applied = self.preview_resize_root_window_to(
                surface_id,
                geometry.width,
                geometry.height,
                geometry.placement,
                edges,
                interaction_id,
            );
            if applied {
                self.queue_backend_configure(
                    window_id,
                    geometry,
                    self.window(window_id)
                        .map(|window| window.state.mode())
                        .unwrap_or(ToplevelMode::Floating),
                    true,
                );
            }
            return applied;
        }
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
        let constraints = self.toplevel_constraints(surface_id);
        let (width, height) = constrain_icccm_size(geometry.width, geometry.height, constraints);
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
        constrain_icccm_dimension(
            width,
            constraints.min_width,
            constraints.max_width,
            constraints.base_width,
            constraints.width_increment,
            MIN_WINDOW_WIDTH,
        )
    }

    pub(in crate::compositor) fn clamp_toplevel_height(&self, surface_id: u32, height: u32) -> u32 {
        let constraints = self.toplevel_constraints(surface_id);
        constrain_icccm_dimension(
            height,
            constraints.min_height,
            constraints.max_height,
            constraints.base_height,
            constraints.height_increment,
            MIN_WINDOW_HEIGHT,
        )
    }

    pub(in crate::compositor) fn toplevel_constraints(
        &self,
        surface_id: u32,
    ) -> ToplevelSizeConstraints {
        self.toplevel_window_constraints(surface_id)
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
        self.resize_flow_debug_event(
            "preview_applied",
            surface_id,
            None,
            None,
            Some(flow_sequence),
            true,
            Some(WindowGeometry::new(placement, width, height)),
        );
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
            self.reconcile_all_surface_output_memberships();
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
        self.reconcile_all_surface_output_memberships();
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
        let Some(window_id) = self.window_id_for_surface(surface_id) else {
            return false;
        };
        if !self.toplevel_surfaces.contains_key(&surface_id) {
            let Some(geometry) = self
                .current_visual_root_window_geometry(surface_id)
                .or_else(|| self.current_root_window_geometry(surface_id))
            else {
                return false;
            };
            self.queue_backend_finalize_resize(
                window_id,
                geometry,
                self.window(window_id)
                    .map(|window| window.state.mode())
                    .unwrap_or(ToplevelMode::Floating),
            );
            return true;
        }
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
        let queued = self
            .resize_configure_flows
            .entry(surface_id)
            .or_default()
            .queue_final(desired);
        self.update_resize_retained_configure_peak(surface_id);
        if queued {
            self.resize_flow_debug_event(
                "final_queued",
                surface_id,
                None,
                None,
                None,
                false,
                Some(WindowGeometry::new(
                    desired.placement,
                    desired.width,
                    desired.height,
                )),
            );
        }
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
        self.resize_flow_debug_event(
            if resize.resizing {
                "configure_sent"
            } else {
                "final_sent"
            },
            surface_id,
            None,
            Some(serial),
            Some(sequence),
            resize.resizing,
            Some(WindowGeometry::new(placement, width, height)),
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

    pub(in crate::compositor) fn finalize_x11_resize(
        &mut self,
        handle: crate::xwayland::X11WindowHandle,
    ) -> bool {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return false;
        };
        let Some(surface_id) = self.window(window_id).map(|window| window.root_surface_id) else {
            return false;
        };
        let Some(active) = self.active_toplevel_resizes.remove(&surface_id) else {
            return false;
        };
        if let Some(visual) = self.toplevel_visual_geometries.get_mut(&surface_id)
            && visual.active_resize == Some(active.interaction_id)
        {
            let placement = visual.placement;
            visual.active_resize = None;
            let _ = self.set_surface_placement_with_cause(
                surface_id,
                placement,
                RenderGenerationCause::WindowResize,
            );
        }
        self.update_toplevel_visual_render_assignment(surface_id);
        true
    }

    pub(in crate::compositor) fn x11_resize_active(
        &self,
        handle: crate::xwayland::X11WindowHandle,
    ) -> bool {
        self.window_id_for_x11_handle(handle)
            .and_then(|window_id| self.window(window_id))
            .is_some_and(|window| {
                self.active_toplevel_resizes
                    .contains_key(&window.root_surface_id)
            })
    }

    pub(in crate::compositor) fn x11_resize_interaction_active(
        &self,
        handle: crate::xwayland::X11WindowHandle,
    ) -> bool {
        let Some(root_surface_id) = self
            .window_id_for_x11_handle(handle)
            .and_then(|window_id| self.window(window_id))
            .map(|window| window.root_surface_id)
        else {
            return false;
        };
        self.window_interaction
            .is_some_and(|interaction| interaction.root_surface_id == root_surface_id)
    }

    pub(in crate::compositor) fn finalize_x11_resize_if_interaction_ended(
        &mut self,
        handle: crate::xwayland::X11WindowHandle,
    ) -> bool {
        if self.x11_resize_interaction_active(handle) {
            return false;
        }
        self.finalize_x11_resize(handle)
    }
}

fn constrain_icccm_size(width: u32, height: u32, constraints: WindowConstraints) -> (u32, u32) {
    let mut width = constrain_icccm_dimension(
        width,
        constraints.min_width,
        constraints.max_width,
        constraints.base_width,
        constraints.width_increment,
        MIN_WINDOW_WIDTH,
    );
    let mut height = constrain_icccm_dimension(
        height,
        constraints.min_height,
        constraints.max_height,
        constraints.base_height,
        constraints.height_increment,
        MIN_WINDOW_HEIGHT,
    );
    if let Some(min_aspect) = constraints.min_aspect.filter(|aspect| *aspect > 0.0)
        && f64::from(width) / f64::from(height) < min_aspect
    {
        width = constrain_icccm_dimension(
            (f64::from(height) * min_aspect).ceil() as u32,
            constraints.min_width,
            constraints.max_width,
            constraints.base_width,
            constraints.width_increment,
            MIN_WINDOW_WIDTH,
        );
    }
    if let Some(max_aspect) = constraints.max_aspect.filter(|aspect| *aspect > 0.0)
        && f64::from(width) / f64::from(height) > max_aspect
    {
        height = constrain_icccm_dimension(
            (f64::from(width) / max_aspect).ceil() as u32,
            constraints.min_height,
            constraints.max_height,
            constraints.base_height,
            constraints.height_increment,
            MIN_WINDOW_HEIGHT,
        );
    }
    (width, height)
}

fn constrain_icccm_dimension(
    requested: u32,
    min: Option<u32>,
    max: Option<u32>,
    base: Option<u32>,
    increment: Option<u32>,
    fallback_min: u32,
) -> u32 {
    let fixed = min
        .zip(max)
        .filter(|(min, max)| min == max)
        .map(|(min, _)| min);
    if let Some(fixed) = fixed {
        return fixed.max(1);
    }
    let lower = min
        .or(base)
        .unwrap_or(if max.is_none() { fallback_min } else { 1 });
    let upper = max.unwrap_or(u32::MAX).max(lower);
    let requested = requested.max(lower).min(upper);
    let Some(increment) = increment.filter(|increment| *increment > 0) else {
        return requested;
    };
    let anchor = base.unwrap_or(lower).min(upper);
    let steps = requested.saturating_sub(anchor) / increment;
    anchor
        .saturating_add(steps.saturating_mul(increment))
        .max(lower)
        .min(upper)
}

#[cfg(test)]
mod icccm_tests {
    use super::*;

    #[test]
    fn x11_resize_respects_base_size_and_increments() {
        let constraints = WindowConstraints {
            min_width: Some(320),
            min_height: Some(200),
            base_width: Some(320),
            base_height: Some(200),
            width_increment: Some(8),
            height_increment: Some(10),
            ..WindowConstraints::default()
        };
        assert_eq!(constrain_icccm_size(327, 209, constraints), (320, 200));
        assert_eq!(constrain_icccm_size(329, 211, constraints), (328, 210));
    }

    #[test]
    fn fixed_size_constraints_win_over_generic_minimum() {
        let constraints = WindowConstraints {
            min_width: Some(100),
            max_width: Some(100),
            min_height: Some(80),
            max_height: Some(80),
            ..WindowConstraints::default()
        };
        assert_eq!(constrain_icccm_size(900, 700, constraints), (100, 80));
    }
}
