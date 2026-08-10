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
            SurfacePlacement::absolute_root_at(geometry.x, geometry.y),
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
        if let Some(root_surface_id) = self.window(window_id).map(|window| window.root_surface_id)
            && let Some(active) = self.active_toplevel_resizes.get(&root_surface_id).copied()
        {
            // Keep this test-only native entry point equivalent to a pointer
            // release: the final compositor visual geometry is sealed before
            // the XWM content transaction is allowed to present.
            let _ = self.preview_resize_root_window_to(
                root_surface_id,
                geometry.width,
                geometry.height,
                SurfacePlacement::absolute_root_at(geometry.x, geometry.y),
                active.edges,
                active.interaction_id,
            );
        }
        self.queue_backend_finalize_resize(
            window_id,
            WindowGeometry::new(
                SurfacePlacement::absolute_root_at(geometry.x, geometry.y),
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
        let render_target_cleared = self
            .renderable_surfaces
            .iter_mut()
            .find(|surface| surface.surface_id == surface_id)
            .and_then(|surface| surface.render_target_size.take())
            .is_some();
        if previous.is_some_and(|visual| {
            visual.width == width
                && visual.height == height
                && visual.placement == placement
                && visual.active_resize == Some(interaction_id)
        }) && !render_target_cleared
        {
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
        if let Some(window_id) = self.window_id_for_surface(surface_id) {
            self.set_x11_frame_geometry(window_id, WindowGeometry::new(placement, width, height));
        }
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
        let root_render_placement = derive_root_render_placement(visual_placement, geometry);
        let clip = render::SurfaceTargetRect::new(
            visual_placement.local_x,
            visual_placement.local_y,
            visual_width,
            visual_height,
        );
        let content_pending = self
            .pending_xwayland_visual_content
            .contains(&root_surface_id);
        let root_surface_info = self
            .renderable_surfaces
            .iter()
            .find(|surface| surface.surface_id == root_surface_id)
            .map(RenderableSurface::buffer_size);
        let visual_clip = (active_resize.is_some() || content_pending).then(|| {
            if active_resize.is_some()
                && let (Some(geometry), Some(root_buffer)) = (geometry, root_surface_info)
            {
                // The aperture is resolved in the root render-placement
                // coordinate space. `surface_render_space_assignments` adds
                // the committed surface origin exactly once when it maps the
                // aperture to output coordinates.
                resolve_root_visual_aperture_for_preview(root_buffer, geometry, clip)
            } else {
                SurfaceVisualAperture::logical_only(clip)
            }
        });
        let placements = &self.surface_placements;
        for surface in &mut self.renderable_surfaces {
            if root_surface_id_for_surface_in_placements(placements, surface.surface_id)
                != root_surface_id
            {
                continue;
            }
            if surface.surface_id == root_surface_id {
                surface.visual_clip = visual_clip.clone();
                surface.render_placement = Some(root_render_placement);
            } else {
                surface.visual_clip = None;
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
        self.finalize_x11_resize_with_geometry(handle, None)
    }

    pub(in crate::compositor) fn finalize_x11_resize_with_geometry(
        &mut self,
        handle: crate::xwayland::X11WindowHandle,
        presented_geometry: Option<crate::xwayland::xwm::X11Geometry>,
    ) -> bool {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return false;
        };
        let Some(surface_id) = self.window(window_id).map(|window| window.root_surface_id) else {
            return false;
        };
        let Some(active) = self.active_toplevel_resizes.get(&surface_id).copied() else {
            return false;
        };
        let Some(visual) = self.toplevel_visual_geometries.get(&surface_id).copied() else {
            self.active_toplevel_resizes.remove(&surface_id);
            return true;
        };
        // A completion event identifies content, not pointer ownership.  If a
        // newer resize has already replaced the interaction ID, this event is
        // for an older content epoch and must not retire the newer preview.
        if visual.active_resize != Some(active.interaction_id) {
            return false;
        }
        if let Some(presented_geometry) = presented_geometry
            && (visual.width != presented_geometry.width
                || visual.height != presented_geometry.height)
        {
            // The XWM event belongs to an older content transaction.  A
            // newer pointer-owned resize has changed the expected content
            // size, so that completion cannot retire the newer preview.
            return false;
        }
        if presented_geometry.is_some_and(|_| {
            self.window_interaction.as_ref().is_some_and(|interaction| {
                interaction.root_surface_id == surface_id
                    && matches!(interaction.kind, WindowInteractionKind::Resize(_))
            })
        }) {
            // A newer pointer-owned resize is still active.  Its visual
            // geometry is a newer epoch than this presentation event.
            return false;
        }
        self.active_toplevel_resizes.remove(&surface_id);
        let placement = visual.placement;
        if let Some(visual) = self.toplevel_visual_geometries.get_mut(&surface_id) {
            visual.active_resize = None;
        }
        self.set_surface_placement_with_cause(
            surface_id,
            placement,
            RenderGenerationCause::WindowResize,
        );
        self.update_pending_xwayland_visual_content(surface_id);
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

pub(crate) fn derive_root_render_placement(
    frame: SurfacePlacement,
    committed_geometry: Option<XdgWindowGeometry>,
) -> SurfacePlacement {
    let Some(committed_geometry) = committed_geometry else {
        return frame;
    };
    SurfacePlacement {
        parent_surface_id: None,
        local_x: frame.local_x.saturating_sub(committed_geometry.x),
        local_y: frame.local_y.saturating_sub(committed_geometry.y),
        root_mode: frame.root_mode,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowVisualExtents {
    pub(crate) left: u32,
    pub(crate) top: u32,
    pub(crate) right: u32,
    pub(crate) bottom: u32,
}

impl WindowVisualExtents {
    pub(crate) fn from_root_buffer_and_window_geometry(
        root_buffer: BufferSize,
        geometry: XdgWindowGeometry,
    ) -> Self {
        let buffer_right = i64::from(root_buffer.width);
        let buffer_bottom = i64::from(root_buffer.height);
        let geometry_right = i64::from(geometry.x).saturating_add(i64::from(geometry.width));
        let geometry_bottom = i64::from(geometry.y).saturating_add(i64::from(geometry.height));
        Self {
            left: non_negative_extent(i64::from(geometry.x)),
            top: non_negative_extent(i64::from(geometry.y)),
            right: non_negative_extent(buffer_right.saturating_sub(geometry_right)),
            bottom: non_negative_extent(buffer_bottom.saturating_sub(geometry_bottom)),
        }
    }
}

fn non_negative_extent(value: i64) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

pub(crate) fn resolve_root_visual_aperture_for_preview(
    root_buffer: BufferSize,
    committed_geometry: XdgWindowGeometry,
    logical_target: SurfaceTargetRect,
) -> SurfaceVisualAperture {
    let extents =
        WindowVisualExtents::from_root_buffer_and_window_geometry(root_buffer, committed_geometry);
    let root_origin = (
        logical_target.x().saturating_sub(committed_geometry.x),
        logical_target.y().saturating_sub(committed_geometry.y),
    );
    SurfaceVisualAperture::for_root_window_preview(
        root_origin,
        root_buffer,
        (extents.left, extents.top, extents.right, extents.bottom),
        logical_target,
    )
}

#[cfg(test)]
mod task_3_red_tests {
    use super::*;

    #[test]
    fn root_render_placement_is_frame_origin_minus_committed_window_geometry() {
        assert_eq!(
            derive_root_render_placement(
                SurfacePlacement::absolute_root_at(100, 100),
                Some(XdgWindowGeometry::new(16, 10, 1000, 700)),
            ),
            SurfacePlacement::absolute_root_at(84, 90),
        );
    }

    #[test]
    fn root_render_placement_derivation_is_deterministic_across_repeated_calculations() {
        let frame = SurfacePlacement::absolute_root_at(100, 100);
        let committed_geometry = Some(XdgWindowGeometry::new(16, 10, 1000, 700));
        let expected = SurfacePlacement::absolute_root_at(84, 90);

        for cycle in 0..100 {
            assert_eq!(
                derive_root_render_placement(frame, committed_geometry),
                expected,
                "derived root placement changed during cycle {cycle}"
            );
        }

        assert_eq!(
            derive_root_render_placement(
                SurfacePlacement::absolute_root_at(120, 130),
                committed_geometry,
            ),
            SurfacePlacement::absolute_root_at(104, 120),
        );
    }

    #[test]
    fn window_visual_extents_use_signed_root_and_xdg_rectangles() {
        let root_buffer = BufferSize::new(332, 242).expect("test root buffer");

        assert_eq!(
            WindowVisualExtents::from_root_buffer_and_window_geometry(
                root_buffer,
                XdgWindowGeometry::new(16, 10, 300, 200),
            ),
            WindowVisualExtents {
                left: 16,
                top: 10,
                right: 16,
                bottom: 32,
            }
        );
        assert_eq!(
            WindowVisualExtents::from_root_buffer_and_window_geometry(
                root_buffer,
                XdgWindowGeometry::new(-12, -8, 300, 200),
            ),
            WindowVisualExtents {
                left: 0,
                top: 0,
                right: 44,
                bottom: 50,
            }
        );
        assert_eq!(
            WindowVisualExtents::from_root_buffer_and_window_geometry(
                root_buffer,
                XdgWindowGeometry::new(40, 20, 360, 260),
            ),
            WindowVisualExtents {
                left: 40,
                top: 20,
                right: 0,
                bottom: 0,
            }
        );
    }

    #[test]
    fn window_visual_extents_clamp_extreme_signed_offsets_without_underflow() {
        let extents = WindowVisualExtents::from_root_buffer_and_window_geometry(
            BufferSize::new(332, 242).expect("test root buffer"),
            XdgWindowGeometry::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX),
        );

        assert_eq!(extents.left, 0);
        assert_eq!(extents.top, 0);
        assert_eq!(extents.right, 333);
        assert_eq!(extents.bottom, 243);
    }

    #[test]
    fn left_edge_preview_preserves_root_extent_and_bounds_logical_content() {
        let aperture = resolve_root_visual_aperture_for_preview(
            BufferSize::new(332, 242).expect("test root buffer"),
            XdgWindowGeometry::new(16, 10, 300, 200),
            SurfaceTargetRect::new(160, 150, 340, 200),
        );

        assert_eq!(
            aperture.logical_target(),
            SurfaceTargetRect::new(160, 150, 340, 200)
        );
        assert_eq!(
            aperture.committed_content_target(),
            Some(SurfaceTargetRect::new(160, 150, 300, 200))
        );
        assert!(
            aperture
                .committed_extent_regions()
                .contains(&SurfaceTargetRect::new(144, 140, 332, 10))
        );
        assert!(
            aperture
                .committed_extent_regions()
                .contains(&SurfaceTargetRect::new(144, 350, 332, 32))
        );
        assert!(
            aperture
                .committed_extent_regions()
                .contains(&SurfaceTargetRect::new(144, 150, 16, 200))
        );
        assert!(
            aperture
                .committed_extent_regions()
                .iter()
                .all(|strip| !strip.intersects(aperture.logical_target()))
        );
    }

    #[test]
    fn top_edge_preview_preserves_top_extent_and_anchor() {
        let aperture = resolve_root_visual_aperture_for_preview(
            BufferSize::new(332, 242).expect("test root buffer"),
            XdgWindowGeometry::new(-12, 10, 300, 200),
            SurfaceTargetRect::new(120, 100, 300, 250),
        );

        assert_eq!(
            aperture.logical_target(),
            SurfaceTargetRect::new(120, 100, 300, 250)
        );
        assert_eq!(
            aperture.committed_content_target(),
            Some(SurfaceTargetRect::new(132, 100, 288, 200))
        );
        assert!(
            aperture
                .committed_extent_regions()
                .iter()
                .any(|strip| strip.y() < 100)
        );
        assert!(
            aperture
                .committed_extent_regions()
                .iter()
                .all(|strip| !strip.intersects(aperture.logical_target()))
        );
    }

    #[test]
    fn top_left_preview_preserves_both_extent_axes_and_resize_anchor() {
        let aperture = resolve_root_visual_aperture_for_preview(
            BufferSize::new(332, 242).expect("test root buffer"),
            XdgWindowGeometry::new(16, 10, 300, 200),
            SurfaceTargetRect::new(140, 120, 260, 170),
        );

        assert_eq!(
            aperture.logical_target(),
            SurfaceTargetRect::new(140, 120, 260, 170)
        );
        assert!(
            aperture
                .committed_extent_regions()
                .iter()
                .any(|strip| strip.x() < 140)
        );
        assert!(
            aperture
                .committed_extent_regions()
                .iter()
                .any(|strip| strip.y() < 120)
        );
        assert!(
            aperture
                .committed_extent_regions()
                .iter()
                .all(|strip| !strip.intersects(aperture.logical_target()))
        );
    }

    #[test]
    fn right_and_bottom_edge_previews_keep_extent_regions_bounded() {
        let aperture = resolve_root_visual_aperture_for_preview(
            BufferSize::new(332, 242).expect("test root buffer"),
            XdgWindowGeometry::new(16, 10, 300, 200),
            SurfaceTargetRect::new(100, 80, 300, 200),
        );

        assert_eq!(
            aperture.committed_content_target(),
            Some(SurfaceTargetRect::new(100, 80, 300, 200))
        );
        assert!(aperture.committed_extent_regions().iter().any(|strip| {
            strip.x()
                >= aperture
                    .logical_target()
                    .x()
                    .saturating_add(i32::try_from(aperture.logical_target().width()).unwrap())
        }));
        assert!(aperture.committed_extent_regions().iter().any(|strip| {
            strip.y()
                >= aperture
                    .logical_target()
                    .y()
                    .saturating_add(i32::try_from(aperture.logical_target().height()).unwrap())
        }));
        assert!(
            aperture
                .committed_extent_regions()
                .iter()
                .all(|strip| !strip.intersects(aperture.logical_target()))
        );
    }

    #[test]
    fn all_corner_grow_and_shrink_previews_keep_anchor_and_clip_stale_content() {
        let root_buffer = BufferSize::new(332, 242).expect("test root buffer");
        let geometry = XdgWindowGeometry::new(16, 10, 300, 200);
        let grown = resolve_root_visual_aperture_for_preview(
            root_buffer,
            geometry,
            SurfaceTargetRect::new(100, 80, 400, 300),
        );
        let shrunk = resolve_root_visual_aperture_for_preview(
            root_buffer,
            geometry,
            SurfaceTargetRect::new(100, 80, 220, 140),
        );

        assert_eq!(
            grown.logical_target(),
            SurfaceTargetRect::new(100, 80, 400, 300)
        );
        assert_eq!(
            grown.committed_content_target(),
            Some(SurfaceTargetRect::new(100, 80, 300, 200))
        );
        assert_eq!(
            shrunk.logical_target(),
            SurfaceTargetRect::new(100, 80, 220, 140)
        );
        assert_eq!(
            shrunk.committed_content_target(),
            Some(SurfaceTargetRect::new(100, 80, 220, 140))
        );
        for aperture in [&grown, &shrunk] {
            assert!(
                aperture
                    .committed_extent_regions()
                    .iter()
                    .all(|strip| !strip.intersects(aperture.logical_target()))
            );
        }
        assert!(shrunk.committed_extent_regions().iter().any(|strip| {
            strip.x()
                >= shrunk
                    .logical_target()
                    .x()
                    .saturating_add(i32::try_from(shrunk.logical_target().width()).unwrap())
        }));
        assert!(shrunk.committed_extent_regions().iter().any(|strip| {
            strip.y()
                >= shrunk
                    .logical_target()
                    .y()
                    .saturating_add(i32::try_from(shrunk.logical_target().height()).unwrap())
        }));
    }

    #[test]
    fn m7_a_csd_extents_survive_one_hundred_edge_and_corner_resize_cycles() {
        let root_buffer = BufferSize::new(332, 242).expect("test root buffer");
        let cases = [
            (
                XdgWindowGeometry::new(16, 10, 300, 200),
                SurfaceTargetRect::new(160, 150, 340, 200),
            ),
            (
                XdgWindowGeometry::new(16, 10, 300, 200),
                SurfaceTargetRect::new(100, 80, 220, 140),
            ),
            (
                XdgWindowGeometry::new(-12, 10, 300, 200),
                SurfaceTargetRect::new(120, 100, 300, 250),
            ),
            (
                XdgWindowGeometry::new(16, -8, 300, 200),
                SurfaceTargetRect::new(120, 100, 300, 250),
            ),
            (
                XdgWindowGeometry::new(-12, -8, 300, 200),
                SurfaceTargetRect::new(140, 120, 260, 170),
            ),
            (
                XdgWindowGeometry::new(16, 10, 300, 200),
                SurfaceTargetRect::new(140, 120, 400, 300),
            ),
            (
                XdgWindowGeometry::new(-12, 10, 300, 200),
                SurfaceTargetRect::new(140, 120, 400, 300),
            ),
            (
                XdgWindowGeometry::new(16, -8, 300, 200),
                SurfaceTargetRect::new(140, 120, 220, 140),
            ),
        ];

        for cycle in 0..100 {
            for (geometry, logical_target) in cases {
                let aperture =
                    resolve_root_visual_aperture_for_preview(root_buffer, geometry, logical_target);
                let root_bounds = SurfaceTargetRect::new(
                    logical_target.x().saturating_sub(geometry.x),
                    logical_target.y().saturating_sub(geometry.y),
                    root_buffer.width,
                    root_buffer.height,
                );
                assert_eq!(aperture.logical_target(), logical_target, "cycle {cycle}");
                assert!(aperture.committed_extent_regions().iter().all(|region| {
                    region.intersection(root_bounds) == Some(*region)
                        && !region.intersects(logical_target)
                }));
            }
        }
    }

    #[test]
    fn root_aperture_does_not_clip_unrelated_subsurfaces_or_leak_stale_content() {
        let root = SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(140, 120, 260, 170));
        let child = SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(0, 0, 40, 40));

        assert!(
            root.content_regions()
                .iter()
                .all(|region| region.intersects(SurfaceTargetRect::new(140, 120, 260, 170)))
        );
        assert_eq!(
            child,
            SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(0, 0, 40, 40))
        );
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
