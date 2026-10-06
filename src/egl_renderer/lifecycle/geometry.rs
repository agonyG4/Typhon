use super::super::*;

pub(super) fn scaled_presentation_rect(
    rect: compositor::PresentationRect,
    output_scale: f64,
) -> compositor::PresentationRect {
    compositor::PresentationRect::new(
        rect.x() * output_scale,
        rect.y() * output_scale,
        rect.width() * output_scale,
        rect.height() * output_scale,
    )
    .unwrap_or(rect)
}

pub(super) fn ndc_to_output_point(
    position: [f32; 2],
    output_size: (u32, u32),
    framebuffer_origin: OutputFramebufferOrigin,
) -> [f64; 2] {
    let x = (f64::from(position[0]) + 1.0) * f64::from(output_size.0) * 0.5;
    let y = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => {
            (1.0 - f64::from(position[1])) * f64::from(output_size.1) * 0.5
        }
        OutputFramebufferOrigin::TopLeftScanout => {
            (f64::from(position[1]) + 1.0) * f64::from(output_size.1) * 0.5
        }
    };
    [x, y]
}

pub(super) fn output_point_to_ndc(
    point: [f64; 2],
    output_size: (u32, u32),
    framebuffer_origin: OutputFramebufferOrigin,
) -> [f32; 2] {
    let x = point[0] / f64::from(output_size.0.max(1)) * 2.0 - 1.0;
    let y = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => {
            1.0 - point[1] / f64::from(output_size.1.max(1)) * 2.0
        }
        OutputFramebufferOrigin::TopLeftScanout => {
            point[1] / f64::from(output_size.1.max(1)) * 2.0 - 1.0
        }
    };
    [x as f32, y as f32]
}

pub(super) fn surface_root_for_lifecycle_surface(
    surface: &RenderableSurface,
    surfaces: &[RenderableSurface],
    lifecycle: &LifecycleSceneSample,
) -> u32 {
    let mut current = surface.surface_id;
    for _ in 0..=surfaces.len() {
        if lifecycle
            .samples
            .iter()
            .any(|sample| sample.root_surface_id == current)
        {
            return current;
        }
        let Some(parent) = surfaces
            .iter()
            .find(|candidate| candidate.surface_id == current)
            .and_then(|candidate| candidate.placement.parent_surface_id)
        else {
            break;
        };
        current = parent;
    }
    current
}
