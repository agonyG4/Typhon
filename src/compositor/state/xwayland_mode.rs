use super::*;

use crate::wm::LayoutMembership;

impl CompositorState {
    pub(in crate::compositor) fn transition_x11_window_mode(
        &mut self,
        window_id: WindowId,
        mode: ToplevelMode,
        minimized: bool,
    ) -> bool {
        self.transition_x11_window_mode_with_target(window_id, mode, minimized, None)
    }

    pub(in crate::compositor) fn transition_x11_window_mode_for_interaction(
        &mut self,
        window_id: WindowId,
        target_geometry: WindowGeometry,
    ) -> bool {
        self.transition_x11_window_mode_with_target(
            window_id,
            ToplevelMode::Normal,
            false,
            Some((target_geometry, VisualGeometryTransition::Immediate)),
        )
    }

    fn transition_x11_window_mode_with_target(
        &mut self,
        window_id: WindowId,
        mode: ToplevelMode,
        minimized: bool,
        interaction_target: Option<(WindowGeometry, VisualGeometryTransition)>,
    ) -> bool {
        let Some((root_surface_id, current_mode, current_minimized, current_geometry)) = self
            .window(window_id)
            .filter(|window| matches!(window.backend, WindowBackend::X11(_)))
            .map(|window| {
                (
                    window.root_surface_id,
                    window.state.mode(),
                    window.state.is_minimized(),
                    window.x11_geometry.map(|geometry| geometry.frame),
                )
            })
        else {
            return false;
        };
        let mode_changed = current_mode != mode;
        let minimized_changed = current_minimized != minimized;
        if !mode_changed && !minimized_changed {
            return false;
        }

        let source_geometry = self
            .toplevel_visual_geometries
            .contains_key(&root_surface_id)
            .then(|| self.current_visual_root_window_geometry(root_surface_id))
            .flatten()
            .or(current_geometry)
            .or_else(|| self.current_visual_root_window_geometry(root_surface_id))
            .or_else(|| self.current_root_window_geometry(root_surface_id))
            .unwrap_or_else(|| WindowGeometry::new(self.surface_placement(root_surface_id), 1, 1));
        let restore_geometry = if mode_changed && mode != ToplevelMode::Normal {
            Some(source_geometry)
        } else {
            None
        };
        if let Some(restore_geometry) = restore_geometry
            && let Some(window) = self.window_mut(window_id)
        {
            window.state.capture_restore_geometry(restore_geometry);
        }

        if (mode_changed || minimized_changed)
            && let Some(location) = self
                .window(window_id)
                .and_then(|window| window.management)
                .filter(|management| management.layout() == LayoutMembership::Tiled)
                .map(|management| management.location())
        {
            self.cancel_tiled_resize_for_location(
                location,
                WindowInteractionEndReason::ModeTransition,
            );
        }
        if mode_changed {
            self.clear_resize_state_for_surfaces_with_reason(
                &[root_surface_id],
                WindowInteractionEndReason::ModeTransition,
            );
        }
        if !minimized && current_minimized {
            self.restore_minimized_desktop_window(window_id);
        }

        let target_geometry = if let Some((target_geometry, _)) = interaction_target {
            target_geometry
        } else if mode == ToplevelMode::Normal && mode_changed {
            self.current_tiled_geometry(window_id)
                .or_else(|| {
                    self.window_mut(window_id)
                        .and_then(|window| window.state.take_restore_geometry())
                })
                .or_else(|| self.current_root_window_geometry(root_surface_id))
                .or(current_geometry)
                .unwrap_or_else(|| {
                    WindowGeometry::new(self.surface_placement(root_surface_id), 1, 1)
                })
        } else {
            self.window_geometry_for_surface_mode(root_surface_id, mode)
        };
        if let Some(window) = self.window_mut(window_id) {
            window.state.set_mode(mode);
        }

        let geometry_changed = current_geometry != Some(target_geometry);
        let transition = interaction_target.map_or_else(
            || {
                mode_transition_animation_kind(current_mode, mode).map_or(
                    VisualGeometryTransition::Immediate,
                    |kind| VisualGeometryTransition::Animated {
                        source: source_geometry,
                        kind,
                    },
                )
            },
            |(_, transition)| transition,
        );
        if geometry_changed || mode_changed || minimized_changed {
            let _ = self.set_x11_frame_geometry(window_id, target_geometry);
            self.set_surface_placement_with_cause(
                root_surface_id,
                target_geometry.placement,
                RenderGenerationCause::WindowMode,
            );
            self.install_x11_visual_geometry_with_transition(
                root_surface_id,
                target_geometry,
                transition,
            );
        }

        if minimized
            && !current_minimized
            && !self.minimize_desktop_window(window_id)
            && let Some(window) = self.window_mut(window_id)
        {
            window.state.mark_minimized_without_surfaces();
        }

        if geometry_changed || mode_changed {
            self.queue_backend_configure(window_id, target_geometry, mode, false);
        }
        if mode == ToplevelMode::Fullscreen && !minimized {
            self.set_fullscreen_presentation_owner(root_surface_id);
        } else {
            self.clear_fullscreen_presentation_owner(root_surface_id);
        }
        self.queue_backend_state(window_id);
        self.mark_astrea_toplevel_dirty(window_id);
        if interaction_target.is_some()
            && let Some(window) = self.window_mut(window_id)
        {
            let _ = window.state.take_restore_geometry();
        }
        true
    }

    pub(in crate::compositor) fn set_x11_frame_geometry(
        &mut self,
        window_id: WindowId,
        geometry: WindowGeometry,
    ) -> bool {
        let Some(window) = self.window_mut(window_id) else {
            return false;
        };
        let Some(x11_geometry) = window.x11_geometry.as_mut() else {
            return false;
        };
        x11_geometry.client = crate::xwayland::xwm::X11Geometry {
            x: geometry.placement.local_x,
            y: geometry.placement.local_y,
            width: geometry.width,
            height: geometry.height,
        };
        x11_geometry.frame = geometry;
        true
    }

    pub(in crate::compositor) fn promote_x11_resize_geometry(
        &mut self,
        handle: crate::xwayland::X11WindowHandle,
        geometry: crate::xwayland::xwm::X11Geometry,
        resize_epoch: u64,
    ) -> bool {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return false;
        };
        let Some(root_surface_id) = self.window(window_id).map(|window| window.root_surface_id)
        else {
            return false;
        };
        let Some(active) = self.active_toplevel_resizes.get(&root_surface_id).copied() else {
            return false;
        };
        let Some(visual) = self.toplevel_visual_geometries.get(&root_surface_id) else {
            return false;
        };
        if active.interaction_id.get() != resize_epoch
            || active.superseded_by_move
            || visual.active_resize != Some(active.interaction_id)
        {
            return false;
        }
        let Some(filtered) = self.filter_x11_geometry(handle, geometry) else {
            return false;
        };
        let placement = SurfacePlacement::absolute_root_at(filtered.x, filtered.y);
        let frame = WindowGeometry::new(placement, filtered.width, filtered.height);
        let Some(window) = self.window_mut(window_id) else {
            return false;
        };
        let placement_policy = window.x11_placement_policy;
        let Some(x11_geometry) = window.x11_geometry.as_mut() else {
            return false;
        };
        x11_geometry.client = if placement_policy == Some(X11PlacementPolicy::CompositorManaged) {
            crate::xwayland::xwm::X11Geometry {
                x: placement.local_x,
                y: placement.local_y,
                ..filtered
            }
        } else {
            filtered
        };
        x11_geometry.frame = frame;
        resize_debug_log(|| {
            format!(
                "event=xwayland_resize_geometry_promoted xid={} resize_epoch={} geometry={:?} visual_preserved=true",
                handle.xid(),
                resize_epoch,
                filtered,
            )
        });
        true
    }

    pub(in crate::compositor) fn install_x11_visual_geometry(
        &mut self,
        root_surface_id: u32,
        geometry: WindowGeometry,
    ) {
        self.install_x11_visual_geometry_with_transition(
            root_surface_id,
            geometry,
            VisualGeometryTransition::Immediate,
        );
    }

    pub(in crate::compositor) fn install_x11_visual_geometry_with_transition(
        &mut self,
        root_surface_id: u32,
        geometry: WindowGeometry,
        transition: VisualGeometryTransition,
    ) {
        let target_cleared = self
            .renderable_surfaces
            .iter_mut()
            .find(|surface| surface.surface_id == root_surface_id)
            .and_then(|surface| surface.render_target_size.take())
            .is_some();
        let changed = self.toplevel_visual_geometries.insert(
            root_surface_id,
            ToplevelVisualGeometry {
                placement: geometry.placement,
                width: geometry.width,
                height: geometry.height,
                active_resize: None,
                mode_transition: false,
                xdg_mode_transition_fence: None,
            },
        ) != Some(ToplevelVisualGeometry {
            placement: geometry.placement,
            width: geometry.width,
            height: geometry.height,
            active_resize: None,
            mode_transition: false,
            xdg_mode_transition_fence: None,
        });
        self.update_pending_xwayland_visual_content(root_surface_id);
        self.update_toplevel_visual_render_assignment(root_surface_id);
        if changed || target_cleared {
            self.advance_render_generation(RenderGenerationCause::WindowMode);
        }
        if changed {
            self.advance_pointer_hit_generation();
            match transition {
                VisualGeometryTransition::Immediate => {
                    self.cancel_presentation_geometry_for_root(root_surface_id);
                }
                VisualGeometryTransition::Animated { source, kind } => {
                    self.animate_toplevel_visual_geometry(root_surface_id, source, geometry, kind);
                }
            }
        }
    }
}
