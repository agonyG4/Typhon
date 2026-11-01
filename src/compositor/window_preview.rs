use super::{
    CompositorState, OwnCompositorServer, PresentationRetainedVisualPayloadId, RenderableSurface,
    SceneNodeId, SurfacePlacement, WindowId,
};
use crate::render_backend::buffer::BufferSize;

/// An exact live window requested by the authenticated shell.
#[derive(Debug, Clone, Copy)]
pub struct WindowPreviewTarget {
    pub window_id: WindowId,
    pub max_width: u32,
    pub max_height: u32,
    /// Set only after the runtime has frozen the selected window's source.
    pub image_size: Option<(u32, u32)>,
    /// The window's unscaled logical size, reported to v3 clients.
    pub window_size: Option<(u32, u32)>,
}

/// Largest preview edge a v3 client may request; older clients keep 1024.
pub const MAX_WINDOW_PREVIEW_EDGE: u32 = 4096;
pub const LEGACY_MAX_WINDOW_PREVIEW_EDGE: u32 = 1024;

/// A frame-local, bounded projection of one committed window surface tree.
#[derive(Debug)]
pub struct WindowPreviewSource {
    pub root_surface_id: u32,
    pub width: u32,
    pub height: u32,
    /// Unscaled logical size of the window the image was fitted from.
    pub window_width: u32,
    pub window_height: u32,
    pub surfaces: Vec<RenderableSurface>,
    pub scene_node_ids: Vec<SceneNodeId>,
    pub retained_resource: Option<PresentationRetainedVisualPayloadId>,
}

/// Capture-only metadata; it never changes the physical output dimensions.
#[derive(Debug, Clone, Copy)]
pub struct WindowPreviewImage {
    pub width: u32,
    pub height: u32,
    pub retained_resource: Option<PresentationRetainedVisualPayloadId>,
}

impl CompositorState {
    pub(in crate::compositor) fn window_preview_source(
        &self,
        window_id: WindowId,
        max_width: u32,
        max_height: u32,
    ) -> Result<WindowPreviewSource, &'static str> {
        if !(1..=MAX_WINDOW_PREVIEW_EDGE).contains(&max_width)
            || !(1..=MAX_WINDOW_PREVIEW_EDGE).contains(&max_height)
        {
            return Err("invalid_size");
        }
        let window = self.window(window_id).ok_or("window_gone")?;
        let root_surface_id = window.root_surface_id;
        if let Some(snapshot) = window.state.minimized_visual_snapshot() {
            let width = snapshot.source_visual_rect.width();
            let height = snapshot.source_visual_rect.height();
            let scale = 1.0_f64
                .min(f64::from(max_width) / width)
                .min(f64::from(max_height) / height);
            return Ok(WindowPreviewSource {
                root_surface_id,
                width: (width * scale).round().clamp(1.0, f64::from(max_width)) as u32,
                height: (height * scale).round().clamp(1.0, f64::from(max_height)) as u32,
                window_width: width.round().max(1.0) as u32,
                window_height: height.round().max(1.0) as u32,
                surfaces: Vec::new(),
                scene_node_ids: Vec::new(),
                retained_resource: Some(snapshot.resource_payload_id),
            });
        }
        // This path includes WindowState's retained minimized surfaces and never
        // promotes them back into the active scene or grants frame callbacks.
        let mut surfaces = self.lifecycle_surface_candidates(window_id, root_surface_id);
        if !surfaces
            .iter()
            .any(|surface| surface.surface_id == root_surface_id)
        {
            return Err("not_ready");
        }
        let scene_node_ids = surfaces
            .iter()
            .map(|surface| {
                self.scene_node_id_for_surface(surface.surface_id)
                    .ok_or("not_ready")
            })
            .collect::<Result<Vec<_>, _>>()?;
        let origins = super::render::surface_origins(&surfaces);
        let mut left = i64::MAX;
        let mut top = i64::MAX;
        let mut right = i64::MIN;
        let mut bottom = i64::MIN;
        for (surface, &(x, y)) in surfaces.iter().zip(&origins) {
            let size = surface.render_target_size.unwrap_or(BufferSize {
                width: surface.width,
                height: surface.height,
            });
            if size.width == 0 || size.height == 0 {
                return Err("not_ready");
            }
            left = left.min(i64::from(x));
            top = top.min(i64::from(y));
            right = right.max(i64::from(x) + i64::from(size.width));
            bottom = bottom.max(i64::from(y) + i64::from(size.height));
        }
        let width = (right - left) as f64;
        let height = (bottom - top) as f64;
        if width <= 0.0 || height <= 0.0 {
            return Err("not_ready");
        }
        let scale = 1.0_f64
            .min(f64::from(max_width) / width)
            .min(f64::from(max_height) / height);
        let image_width = (width * scale).round().clamp(1.0, f64::from(max_width)) as u32;
        let image_height = (height * scale).round().clamp(1.0, f64::from(max_height)) as u32;
        for (surface, &(x, y)) in surfaces.iter_mut().zip(&origins) {
            let size = surface.render_target_size.unwrap_or(BufferSize {
                width: surface.width,
                height: surface.height,
            });
            let x = ((i64::from(x) - left) as f64 * scale).round() as i32;
            let y = ((i64::from(y) - top) as f64 * scale).round() as i32;
            // Flatten only the cloned presentation. Buffer dimensions, viewport
            // sampling and the compositor's canonical surface tree stay intact.
            surface.x = 0;
            surface.y = 0;
            surface.placement = SurfacePlacement::absolute_root_at(x, y);
            surface.render_placement = None;
            surface.visual_clip = None;
            surface.render_target_size = BufferSize::new(
                (f64::from(size.width) * scale).round().max(1.0) as u32,
                (f64::from(size.height) * scale).round().max(1.0) as u32,
            );
        }
        Ok(WindowPreviewSource {
            root_surface_id,
            width: image_width,
            height: image_height,
            window_width: width as u32,
            window_height: height as u32,
            surfaces,
            scene_node_ids,
            retained_resource: None,
        })
    }
}

impl OwnCompositorServer {
    pub fn window_preview_source(
        &self,
        window_id: WindowId,
        max_width: u32,
        max_height: u32,
    ) -> Result<WindowPreviewSource, &'static str> {
        self.state
            .window_preview_source(window_id, max_width, max_height)
    }
}
