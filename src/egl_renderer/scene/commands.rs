use oblivion_one::compositor::{
    self, DecorationRenderInstance, DecorationRenderPrimitive, RenderableSurface,
    SurfaceOpaqueRect, SurfaceOpaqueRegion, VisualGroupId, clipped_decoration_text_geometry,
};

use super::super::{
    OutputFramebufferOrigin,
    geometry::{
        EglDrawCommand, EglDrawLayer, EglRect, EglTexturedVertex, EglUvRect, SurfaceSampling,
        push_draw_command, push_draw_command_with_uv, surface_sampling_for_plan,
    },
};

pub(in crate::egl_renderer) fn push_output_background_command(
    vertices: &mut Vec<EglTexturedVertex>,
    commands: &mut Vec<EglDrawCommand>,
    width: u32,
    height: u32,
    framebuffer_origin: OutputFramebufferOrigin,
) {
    push_draw_command(
        vertices,
        commands,
        EglDrawLayer::Solid(compositor::ServerFrameColor::OutputBackground),
        EglRect::new(0.0, 0.0, width as f32, height as f32),
        width,
        height,
        framebuffer_origin,
    );
}

pub(in crate::egl_renderer) fn push_egl_decoration_instance(
    vertices: &mut Vec<EglTexturedVertex>,
    commands: &mut Vec<EglDrawCommand>,
    output_width: u32,
    output_height: u32,
    instance: &DecorationRenderInstance,
    output_scale: f64,
    framebuffer_origin: OutputFramebufferOrigin,
    visual_group: Option<VisualGroupId>,
) {
    let command_start = commands.len();
    for primitive in instance.primitives() {
        match primitive {
            DecorationRenderPrimitive::SolidRect { rect, color } => {
                push_egl_decoration_rect(
                    vertices,
                    commands,
                    output_width,
                    output_height,
                    EglDrawLayer::SolidRgba(rgba_to_pixel(*color)),
                    instance,
                    *rect,
                    output_scale,
                    framebuffer_origin,
                );
            }
            DecorationRenderPrimitive::Image { rect, asset } => {
                push_egl_decoration_rect(
                    vertices,
                    commands,
                    output_width,
                    output_height,
                    EglDrawLayer::DecorationAsset(asset.asset_id()),
                    instance,
                    *rect,
                    output_scale,
                    framebuffer_origin,
                );
            }
            DecorationRenderPrimitive::Text {
                rect, clip, asset, ..
            } => push_egl_decoration_text(
                vertices,
                commands,
                output_width,
                output_height,
                instance,
                *rect,
                *clip,
                asset,
                output_scale,
                framebuffer_origin,
            ),
        }
    }
    for command in &mut commands[command_start..] {
        command.visual_group = visual_group;
    }
}

#[allow(clippy::too_many_arguments)]
fn push_egl_decoration_rect(
    vertices: &mut Vec<EglTexturedVertex>,
    commands: &mut Vec<EglDrawCommand>,
    output_width: u32,
    output_height: u32,
    layer: EglDrawLayer,
    instance: &DecorationRenderInstance,
    rect: oblivion_one::compositor::DecorationRect,
    output_scale: f64,
    framebuffer_origin: OutputFramebufferOrigin,
) {
    let scale = output_scale.max(1.0) as f32;
    let (origin_x, origin_y) = instance.origin();
    let x = (origin_x.saturating_add(rect.x) as f32) * scale;
    let y = (origin_y.saturating_add(rect.y) as f32) * scale;
    push_draw_command(
        vertices,
        commands,
        layer,
        EglRect::new(x, y, rect.width as f32 * scale, rect.height as f32 * scale),
        output_width,
        output_height,
        framebuffer_origin,
    );
}

#[allow(clippy::too_many_arguments)]
fn push_egl_decoration_text(
    vertices: &mut Vec<EglTexturedVertex>,
    commands: &mut Vec<EglDrawCommand>,
    output_width: u32,
    output_height: u32,
    instance: &DecorationRenderInstance,
    rect: oblivion_one::compositor::DecorationRect,
    clip: oblivion_one::compositor::DecorationRect,
    asset: &oblivion_one::compositor::DecorationRasterAsset,
    output_scale: f64,
    framebuffer_origin: OutputFramebufferOrigin,
) {
    let scale = output_scale.max(1.0) as f32;
    let (origin_x, origin_y) = instance.origin();
    let Some(crop) = clipped_decoration_text_geometry(rect, clip) else {
        return;
    };
    let crop_rect = crop.rect;
    let uv = EglUvRect::new(crop.uv[0], crop.uv[1], crop.uv[2], crop.uv[3]);
    push_draw_command_with_uv(
        vertices,
        commands,
        EglDrawLayer::DecorationAsset(asset.asset_id()),
        EglRect::new(
            (origin_x.saturating_add(crop_rect.x) as f32) * scale,
            (origin_y.saturating_add(crop_rect.y) as f32) * scale,
            crop_rect.width as f32 * scale,
            crop_rect.height as f32 * scale,
        ),
        uv,
        SurfaceSampling::ScaledLinear,
        output_width,
        output_height,
        framebuffer_origin,
    );
}

pub(in crate::egl_renderer) fn rgba_to_pixel(color: [u8; 4]) -> u32 {
    (u32::from(color[3]) << 24)
        | (u32::from(color[0]) << 16)
        | (u32::from(color[1]) << 8)
        | u32::from(color[2])
}

#[allow(clippy::too_many_arguments)]
pub(in crate::egl_renderer) fn push_egl_surface_commands(
    vertices: &mut Vec<EglTexturedVertex>,
    commands: &mut Vec<EglDrawCommand>,
    width: u32,
    height: u32,
    surface: &RenderableSurface,
    render_assignment: compositor::SurfaceRenderSpaceAssignment,
    framebuffer_origin: OutputFramebufferOrigin,
    visual_group: Option<VisualGroupId>,
) {
    let command_start = commands.len();
    if let Some(bounds) =
        compositor::xwayland_visual_backing_target(surface, render_assignment.visual_clip.as_ref())
    {
        push_draw_command(
            vertices,
            commands,
            EglDrawLayer::Solid(compositor::ServerFrameColor::XwaylandBacking),
            EglRect::new(
                bounds.x() as f32,
                bounds.y() as f32,
                bounds.width() as f32,
                bounds.height() as f32,
            ),
            width,
            height,
            framebuffer_origin,
        );
    }
    for render_plan in compositor::surface_render_plans_with_aperture(
        surface,
        render_assignment.target,
        render_assignment.visual_clip.as_ref(),
    ) {
        push_egl_render_plan(
            vertices,
            commands,
            width,
            height,
            surface,
            render_plan,
            framebuffer_origin,
        );
    }
    for command in &mut commands[command_start..] {
        command.visual_group = visual_group;
    }
}

fn push_egl_render_plan(
    vertices: &mut Vec<EglTexturedVertex>,
    commands: &mut Vec<EglDrawCommand>,
    width: u32,
    height: u32,
    surface: &RenderableSurface,
    render_plan: compositor::SurfaceRenderPlan,
    framebuffer_origin: OutputFramebufferOrigin,
) {
    let uv = EglUvRect::from_surface_uv_quad(render_plan.content_uv);
    let sampling = surface_sampling_for_plan(
        surface.buffer_size().width,
        surface.buffer_size().height,
        render_plan.content_target.x(),
        render_plan.content_target.y(),
        render_plan.content_target.width(),
        render_plan.content_target.height(),
        uv,
    );
    push_draw_command_with_uv(
        vertices,
        commands,
        EglDrawLayer::Surface(surface.surface_id),
        EglRect::new(
            render_plan.content_target.x() as f32,
            render_plan.content_target.y() as f32,
            render_plan.content_target.width() as f32,
            render_plan.content_target.height() as f32,
        ),
        uv,
        sampling,
        width,
        height,
        framebuffer_origin,
    );
    if let Some(command) = commands.last_mut()
        && command.layer == EglDrawLayer::Surface(surface.surface_id)
    {
        command.opaque_regions = opaque_region_output_rects(surface, render_plan);
    }
}

fn opaque_region_output_rects(
    surface: &RenderableSurface,
    render_plan: compositor::SurfaceRenderPlan,
) -> Vec<EglRect> {
    match surface.opaque_region() {
        SurfaceOpaqueRegion::None => Vec::new(),
        SurfaceOpaqueRegion::Full => vec![egl_rect_from_target(render_plan.content_target)],
        SurfaceOpaqueRegion::Partial(rects) => rects
            .iter()
            .filter_map(|rect| map_opaque_rect_to_output(surface, render_plan, *rect))
            .collect(),
    }
}

fn egl_rect_from_target(target: compositor::SurfaceTargetRect) -> EglRect {
    EglRect::new(
        target.x() as f32,
        target.y() as f32,
        target.width() as f32,
        target.height() as f32,
    )
}

fn map_opaque_rect_to_output(
    surface: &RenderableSurface,
    render_plan: compositor::SurfaceRenderPlan,
    opaque: SurfaceOpaqueRect,
) -> Option<EglRect> {
    if surface.width == 0 || surface.height == 0 {
        return None;
    }
    let visual = render_plan.visual_target;
    let logical_x = f64::from(opaque.x());
    let logical_y = f64::from(opaque.y());
    let logical_right = logical_x + f64::from(opaque.width());
    let logical_bottom = logical_y + f64::from(opaque.height());
    let scale_x = f64::from(visual.width()) / f64::from(surface.width);
    let scale_y = f64::from(visual.height()) / f64::from(surface.height);
    let left = f64::from(visual.x()) + logical_x * scale_x;
    let top = f64::from(visual.y()) + logical_y * scale_y;
    let right = f64::from(visual.x()) + logical_right * scale_x;
    let bottom = f64::from(visual.y()) + logical_bottom * scale_y;
    let clip = render_plan.content_target;
    let clipped_left = left.max(f64::from(clip.x()));
    let clipped_top = top.max(f64::from(clip.y()));
    let clipped_right = right.min(f64::from(clip.x()) + f64::from(clip.width()));
    let clipped_bottom = bottom.min(f64::from(clip.y()) + f64::from(clip.height()));
    (clipped_right > clipped_left && clipped_bottom > clipped_top).then_some(EglRect::new(
        clipped_left as f32,
        clipped_top as f32,
        (clipped_right - clipped_left) as f32,
        (clipped_bottom - clipped_top) as f32,
    ))
}
