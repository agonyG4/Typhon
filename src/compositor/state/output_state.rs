use super::*;

impl CompositorState {
    /// The native output's layout entry built from the scalar output fields.
    pub(in crate::compositor) fn native_logical_output(&self) -> LogicalOutput {
        LogicalOutput {
            size: self.output_size,
            scale: self.output_scale,
            refresh: self.output_refresh,
            physical_size_mm: self.output_physical_size_mm,
            ..LogicalOutput::native()
        }
    }

    pub(in crate::compositor) fn native_logical_output_mut(
        &mut self,
    ) -> Option<&mut LogicalOutput> {
        let output_id = self.native_output_id()?;
        self.logical_outputs.get_mut(&output_id)
    }

    pub(in crate::compositor) fn logical_output(
        &self,
        output_id: OutputId,
    ) -> Option<&LogicalOutput> {
        self.logical_outputs.get(&output_id)
    }

    /// Adds a logical output next to the native one and advertises its global.
    pub(in crate::compositor) fn add_logical_output(
        &mut self,
        display: &DisplayHandle,
        output: LogicalOutput,
    ) -> Option<OutputId> {
        let output_id = self.output_id_allocator.allocate().ok()?;
        self.logical_output_ids.insert(output_id);
        self.logical_outputs.insert(output_id, output);
        self.publish_output_global(display, output_id);
        // The new output shows a workspace that was hidden until now.
        if self.workspace_manager.attach_output(output_id).is_some() {
            self.workspace_presentation.clear();
            self.rebuild_active_scene_view();
            self.advance_render_generation(RenderGenerationCause::OutputChange);
            self.publish_workspace_state();
        }
        self.relayout_for_output_topology_change();
        self.reconcile_all_surface_output_memberships();
        Some(output_id)
    }

    /// Moves a logical output in layout space and re-announces its geometry.
    pub(in crate::compositor) fn set_logical_output_position(
        &mut self,
        output_id: OutputId,
        position: (i32, i32),
    ) -> bool {
        let Some(output) = self.logical_outputs.get_mut(&output_id) else {
            return false;
        };
        if output.position == position {
            return false;
        }
        output.position = position;
        let output = output.clone();
        for binding in &self.output_resources {
            if binding.output_id == output_id {
                output::send_output_geometry(&binding.resource, &output);
                send_output_done_if_supported(&binding.resource);
            }
        }
        self.send_xdg_output_geometry_to_bound_outputs();
        self.relayout_for_output_topology_change();
        self.reconcile_all_surface_output_memberships();
        true
    }

    /// Re-arranges layer surfaces and every workspace's windows after an output
    /// appeared, disappeared or moved.
    pub(in crate::compositor) fn relayout_for_output_topology_change(&mut self) {
        self.reconfigure_layer_surfaces_for_output_change();
        let _ = self.reflow_usable_output_geometry();
    }

    pub(in crate::compositor) fn set_output_physical_size_mm(
        &mut self,
        size_mm: Option<(u32, u32)>,
    ) -> bool {
        // Wayland uses zero for unknown physical dimensions. Do not invent a DPI.
        let size = size_mm
            .filter(|&(width, height)| width > 0 && height > 0)
            .and_then(|(width, height)| {
                Some((i32::try_from(width).ok()?, i32::try_from(height).ok()?))
            })
            .unwrap_or((0, 0));
        if self.output_physical_size_mm == size {
            return false;
        }
        self.output_physical_size_mm = size;
        let Some(native_output_id) = self.native_output_id() else {
            return true;
        };
        let Some(native) = self.native_logical_output_mut() else {
            return true;
        };
        native.physical_size_mm = size;
        let native = native.clone();
        for binding in &self.output_resources {
            if binding.output_id == native_output_id {
                output::send_output_geometry(&binding.resource, &native);
                send_output_done_if_supported(&binding.resource);
            }
        }
        true
    }

    pub(in crate::compositor) fn set_output_scale_factor(&mut self, scale_factor: f64) -> bool {
        let output_scale = OutputScale::from_factor(scale_factor);
        if self.output_scale == output_scale {
            return false;
        }

        self.output_scale = output_scale;
        if let Some(native) = self.native_logical_output_mut() {
            native.scale = output_scale;
        }
        self.send_output_scale_to_bound_outputs();
        self.send_fractional_scale_to_bound_surfaces();
        self.reconcile_all_surface_output_memberships();
        self.advance_render_generation(RenderGenerationCause::OutputChange);
        true
    }

    pub(in crate::compositor) fn set_output_preferred_transform(
        &mut self,
        transform: wl_output::Transform,
    ) -> bool {
        if self.preferred_output_transform == Some(transform) {
            return false;
        }
        self.preferred_output_transform = Some(transform);
        self.reconcile_all_surface_output_memberships();
        self.advance_render_generation(RenderGenerationCause::OutputChange);
        true
    }
}
