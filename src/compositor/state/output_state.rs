use super::*;

impl CompositorState {
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
        for binding in &self.output_resources {
            output::send_output_geometry(&binding.resource, size);
            send_output_done_if_supported(&binding.resource);
        }
        true
    }

    pub(in crate::compositor) fn set_output_scale_factor(&mut self, scale_factor: f64) -> bool {
        let output_scale = OutputScale::from_factor(scale_factor);
        if self.output_scale == output_scale {
            return false;
        }

        self.output_scale = output_scale;
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
