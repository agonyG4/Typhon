use super::*;

impl CompositorState {
    pub(in crate::compositor) fn transition_x11_window_mode(
        &mut self,
        window_id: WindowId,
        mode: ToplevelMode,
        minimized: bool,
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

        let restore_geometry = if mode_changed && mode != ToplevelMode::Floating {
            self.current_visual_root_window_geometry(root_surface_id)
                .or_else(|| self.current_root_window_geometry(root_surface_id))
                .or(current_geometry)
        } else {
            None
        };
        if let Some(restore_geometry) = restore_geometry
            && let Some(window) = self.window_mut(window_id)
        {
            window.state.capture_restore_geometry(restore_geometry);
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

        let target_geometry = if mode == ToplevelMode::Floating && mode_changed {
            self.window_mut(window_id)
                .and_then(|window| window.state.take_restore_geometry())
                .or_else(|| self.current_root_window_geometry(root_surface_id))
                .or(current_geometry)
                .unwrap_or_else(|| {
                    WindowGeometry::new(self.surface_placement(root_surface_id), 1, 1)
                })
        } else {
            self.window_geometry_for_mode(mode)
        };
        if let Some(window) = self.window_mut(window_id) {
            window.state.set_mode(mode);
        }

        let geometry_changed = current_geometry != Some(target_geometry);
        if geometry_changed || mode_changed || minimized_changed {
            let _ = self.set_x11_frame_geometry(window_id, target_geometry);
            self.set_surface_placement_with_cause(
                root_surface_id,
                target_geometry.placement,
                RenderGenerationCause::WindowMode,
            );
            self.install_x11_visual_geometry(root_surface_id, target_geometry);
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

    pub(in crate::compositor) fn install_x11_visual_geometry(
        &mut self,
        root_surface_id: u32,
        geometry: WindowGeometry,
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
            },
        ) != Some(ToplevelVisualGeometry {
            placement: geometry.placement,
            width: geometry.width,
            height: geometry.height,
            active_resize: None,
        });
        self.update_pending_xwayland_visual_content(root_surface_id);
        self.update_toplevel_visual_render_assignment(root_surface_id);
        if changed || target_cleared {
            self.advance_render_generation(RenderGenerationCause::WindowMode);
        }
    }
}
