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
            |surface, target| self.presentation_coverage_opacity(surface, target, output_size),
        )
    }

    fn presentation_coverage_opacity(
        &self,
        surface: &RenderableSurface,
        target: SurfaceTargetRect,
        output_size: BufferSize,
    ) -> PresentationCoverageOpacity {
        let output_target = SurfaceTargetRect::new(0, 0, output_size.width, output_size.height);
        let proven = target.intersection(output_target) == Some(output_target)
            && surface.buffer_source() == SurfaceBufferSource::Dmabuf
            && surface
                .dmabuf_handle()
                .is_some_and(|buffer| buffer.format() == DrmFormat::Xrgb8888)
            && surface
                .dmabuf_handle()
                .is_some_and(|buffer| buffer.size() == output_size)
            && surface.visual_clip.is_none()
            && surface
                .render_placement
                .is_none_or(|placement| placement == surface.placement)
            && surface.render_target_size.is_none();
        if proven {
            PresentationCoverageOpacity::OpaqueXrgb8888
        } else {
            PresentationCoverageOpacity::Unknown
        }
    }
}
