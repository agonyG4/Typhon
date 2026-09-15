use super::*;
use crate::compositor::presentation_coverage::{
    PresentationCoverageAnalysis, PresentationCoverageOpacity, analyze_presentation_coverage,
};
use crate::render_backend::buffer::{BufferSize, DrmFormat, SurfaceBufferSource};

impl CompositorState {
    pub(in crate::compositor) fn presentation_coverage_analysis(
        &self,
    ) -> PresentationCoverageAnalysis {
        let output_size = BufferSize::new(self.output_size.width, self.output_size.height)
            .expect("configured output size is nonzero");
        let active_surfaces = self.active_scene_surfaces();
        let origins = self.active_scene_surface_origins();
        let render_targets =
            crate::compositor::render::surface_render_space_targets(active_surfaces, origins, 1.0);
        let decorations = self.native_decoration_render_instances_for_scale_with_origins(
            active_surfaces,
            origins,
            1.0,
        );

        analyze_presentation_coverage(
            active_surfaces,
            &decorations,
            &render_targets,
            self.active_scene_popup_surface_ids(),
            output_size,
            |root_surface_id| self.window_id_for_surface(root_surface_id).is_some(),
            |root_surface_id| self.layer_surfaces.contains_key(&root_surface_id),
            |root_surface_id| {
                self.current_visual_root_window_geometry(root_surface_id)
                    .is_some_and(|geometry| {
                        geometry.width == self.output_size.width
                            && geometry.height == self.output_size.height
                            && geometry.placement == SurfacePlacement::absolute_root_at(0, 0)
                    })
            },
            |root_surface_id| self.presentation_coverage_opacity(root_surface_id, output_size),
        )
    }

    fn presentation_coverage_opacity(
        &self,
        root_surface_id: u32,
        output_size: BufferSize,
    ) -> PresentationCoverageOpacity {
        let Some(root) = self
            .active_scene_surfaces()
            .iter()
            .find(|surface| surface.surface_id == root_surface_id)
        else {
            return PresentationCoverageOpacity::Unknown;
        };
        let proven = root.buffer_source() == SurfaceBufferSource::Dmabuf
            && root
                .dmabuf_handle()
                .is_some_and(|buffer| buffer.format() == DrmFormat::Xrgb8888)
            && root
                .dmabuf_handle()
                .is_some_and(|buffer| buffer.size() == output_size)
            && root.visual_clip.is_none()
            && root
                .render_placement
                .is_none_or(|placement| placement == root.placement)
            && root.render_target_size.is_none()
            && root.placement == SurfacePlacement::absolute_root_at(0, 0);
        if proven {
            PresentationCoverageOpacity::OpaqueXrgb8888
        } else {
            PresentationCoverageOpacity::Unknown
        }
    }
}
