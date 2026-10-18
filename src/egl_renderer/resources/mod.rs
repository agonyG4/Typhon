mod dmabuf_import;
mod dmabuf_registry;
mod image;
pub(in crate::egl_renderer) mod surface;
#[cfg(test)]
mod tests;
mod ui;
mod upload;

pub(crate) use image::EglImageGuard;

use std::collections::HashMap;

use khronos_egl as egl;
use oblivion_one::{compositor, cursor_theme::CompositorCursorImage};

#[cfg(test)]
use super::EglDrawLayer;
use super::{EglInstance, GlEglImageTargetTexture2DOes, GlesSceneFrameStats, RendererResult};
use dmabuf_registry::DmabufResourceState;
use image::EglImageResource;
use surface::EglSurfaceResource;
use surface::SurfaceResourceStore;
use ui::DecorationResourceKey;
use ui::UiResourceStore;
use upload::UploadScratch;

/// One read-only borrowed route to ordinary compositor-owned textures.
/// Lifecycle retained textures are resolved separately through EffectRuntime.
#[derive(Clone, Copy)]
pub(in crate::egl_renderer) struct ResourceTextureView<'a> {
    surfaces: &'a HashMap<u32, EglSurfaceResource>,
    dmabuf_registry: &'a DmabufResourceState,
    frames: &'a HashMap<compositor::ServerFrameColor, EglImageResource>,
    decorations: &'a HashMap<DecorationResourceKey, EglImageResource>,
    cursor: Option<&'a EglImageResource>,
}

impl ResourceTextureView<'_> {
    pub(in crate::egl_renderer) fn texture_for_surface(
        &self,
        surface_id: u32,
    ) -> Option<glow::Texture> {
        match self.surfaces.get(&surface_id)? {
            EglSurfaceResource::Uploaded { image, .. } => Some(image.texture),
            EglSurfaceResource::Dmabuf { key } => self.dmabuf_registry.texture_for_key(key),
        }
    }

    pub(in crate::egl_renderer) fn texture_for_frame(
        &self,
        color: compositor::ServerFrameColor,
    ) -> Option<glow::Texture> {
        self.frames.get(&color).map(|resource| resource.texture)
    }

    pub(in crate::egl_renderer) fn texture_for_solid_decoration(
        &self,
        color: u32,
    ) -> Option<glow::Texture> {
        self.decorations
            .get(&DecorationResourceKey::Solid(color))
            .map(|resource| resource.texture)
    }

    pub(in crate::egl_renderer) fn texture_for_decoration_asset(
        &self,
        asset_id: u64,
    ) -> Option<glow::Texture> {
        self.decorations
            .get(&DecorationResourceKey::Asset(asset_id))
            .map(|resource| resource.texture)
    }

    pub(in crate::egl_renderer) fn cursor_texture(&self) -> Option<glow::Texture> {
        self.cursor.map(|resource| resource.texture)
    }

    pub(in crate::egl_renderer) fn cursor_size(&self) -> Option<(u32, u32)> {
        self.cursor.map(|resource| resource.size)
    }
}

/// The single renderer-side owner for ordinary surface, DMA-BUF, and UI
/// image resources. GL/EGL handles remain operation arguments.
pub(super) struct RendererResourceState {
    surfaces: SurfaceResourceStore,
    dmabuf: DmabufResourceState,
    ui: UiResourceStore,
    upload: UploadScratch,
}

impl RendererResourceState {
    pub(super) fn new(image_target_texture_2d: Option<GlEglImageTargetTexture2DOes>) -> Self {
        Self {
            surfaces: SurfaceResourceStore::default(),
            dmabuf: DmabufResourceState::new(image_target_texture_2d),
            ui: UiResourceStore::default(),
            upload: UploadScratch::default(),
        }
    }

    pub(in crate::egl_renderer) fn texture_view(&self) -> ResourceTextureView<'_> {
        ResourceTextureView {
            surfaces: &self.surfaces.resources,
            dmabuf_registry: &self.dmabuf,
            frames: &self.ui.frames,
            decorations: &self.ui.decorations,
            cursor: self.ui.cursor.as_ref(),
        }
    }

    pub(super) fn mark_cursor_stale(&mut self) {
        self.ui.mark_cursor_stale();
    }

    pub(super) fn ensure_cursor_resource(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
        cursor_image: &CompositorCursorImage,
    ) -> RendererResult<()> {
        self.ui
            .ensure_cursor_resource(gl, egl, egl_display, cursor_image, &mut self.upload)
    }

    pub(super) fn ensure_frame_resources(&mut self, gl: &glow::Context) -> RendererResult<()> {
        self.ui.ensure_frame_resources(gl, &mut self.upload)
    }

    pub(super) fn ensure_decoration_resources<'a, I>(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
        instances: I,
    ) -> RendererResult<()>
    where
        I: IntoIterator<Item = &'a compositor::DecorationRenderInstance>,
    {
        self.ui
            .ensure_decoration_resources(gl, egl, egl_display, instances, &mut self.upload)
    }

    pub(super) fn capture_snapshot(&self) -> RendererResourceCaptureSnapshot {
        RendererResourceCaptureSnapshot {
            failed_surface_generations: self.dmabuf.failed_surface_generations.clone(),
        }
    }

    pub(super) fn restore_capture_snapshot(&mut self, snapshot: RendererResourceCaptureSnapshot) {
        self.dmabuf.failed_surface_generations = snapshot.failed_surface_generations;
    }

    pub(super) fn destroy(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
    ) {
        self.ui.destroy_cursor(gl, egl, egl_display);
        self.ui.destroy_frames(gl, egl, egl_display);
        self.ui.destroy_decorations(gl, egl, egl_display);
        self.surfaces.destroy_all(gl, egl, egl_display);
        self.dmabuf.destroy_all(gl, egl, egl_display);
    }

    #[cfg(test)]
    pub(super) fn test_configure_texture(gl: &glow::Context) {
        image::configure_texture(gl);
    }

    #[cfg(test)]
    pub(super) fn test_install_surface_texture(&mut self, surface_id: u32, texture: glow::Texture) {
        self.surfaces.resources.insert(
            surface_id,
            EglSurfaceResource::Uploaded {
                image: image::EglImageResource {
                    texture,
                    size: (1, 1),
                    generation: 1,
                    egl_image: None,
                    telemetry: image::EglImageResourceTelemetry::Untracked,
                },
                shm_synced_commit: None,
            },
        );
    }

    #[cfg(test)]
    fn test_install_surface_image(&mut self, surface_id: u32, image: EglImageResource) {
        self.surfaces.resources.insert(
            surface_id,
            EglSurfaceResource::Uploaded {
                image,
                shm_synced_commit: None,
            },
        );
    }

    #[cfg(test)]
    pub(super) fn test_create_surface_texture(
        &mut self,
        gl: &glow::Context,
        surface_id: u32,
        width: u32,
        height: u32,
        rgba: Option<&[u8]>,
    ) -> RendererResult<glow::Texture> {
        let image = image::create_uploaded_resource(
            gl,
            width,
            height,
            crate::gpu_residency::GpuResourceClass::ShmUploadTextureBacking,
        )?;
        if let Some(rgba) = rgba {
            image::write_rgba_bytes_to_resource(
                gl,
                &image,
                oblivion_one::compositor::SurfaceDamageRect::full(width, height),
                rgba,
            );
        }
        let texture = image.texture;
        self.test_install_surface_image(surface_id, image);
        Ok(texture)
    }

    #[cfg(test)]
    fn test_install_decoration_image(
        &mut self,
        key: DecorationResourceKey,
        image: EglImageResource,
    ) {
        self.ui.decorations.insert(key, image);
    }

    #[cfg(test)]
    pub(super) fn test_create_decoration_texture(
        &mut self,
        gl: &glow::Context,
        layer: EglDrawLayer,
        width: u32,
        height: u32,
        rgba: Option<&[u8]>,
    ) -> RendererResult<glow::Texture> {
        let image = image::create_uploaded_resource(
            gl,
            width,
            height,
            crate::gpu_residency::GpuResourceClass::UiTextureBacking,
        )?;
        if let Some(rgba) = rgba {
            image::write_rgba_bytes_to_resource(
                gl,
                &image,
                oblivion_one::compositor::SurfaceDamageRect::full(width, height),
                rgba,
            );
        }
        let key = match layer {
            EglDrawLayer::SolidRgba(color) => DecorationResourceKey::Solid(color),
            EglDrawLayer::DecorationAsset(asset_id) => DecorationResourceKey::Asset(asset_id),
            _ => panic!("test texture helper requires a decoration draw layer"),
        };
        let texture = image.texture;
        self.test_install_decoration_image(key, image);
        Ok(texture)
    }

    #[cfg(test)]
    pub(super) fn test_write_surface_rgba(
        &self,
        gl: &glow::Context,
        surface_id: u32,
        rect: oblivion_one::compositor::SurfaceDamageRect,
        rgba: &[u8],
    ) -> usize {
        let resource = self
            .surfaces
            .resources
            .get(&surface_id)
            .expect("test surface texture exists");
        let EglSurfaceResource::Uploaded { image, .. } = resource else {
            panic!("test surface resource is an uploaded image");
        };
        image::write_rgba_bytes_to_resource(gl, image, rect, rgba)
    }

    #[cfg(test)]
    pub(super) fn test_install_frame_texture(
        &mut self,
        color: compositor::ServerFrameColor,
        texture: glow::Texture,
    ) {
        self.ui.test_install_frame_texture(color, texture);
    }

    #[cfg(test)]
    pub(super) fn test_install_decoration_texture(
        &mut self,
        layer: EglDrawLayer,
        texture: glow::Texture,
    ) {
        self.ui.test_install_decoration_texture(layer, texture);
    }

    #[cfg(test)]
    pub(super) fn test_install_cursor_texture(&mut self, texture: glow::Texture) {
        self.ui.test_install_cursor_texture(texture);
    }

    #[cfg(test)]
    pub(super) fn test_set_failed_surface_generation(&mut self, surface_id: u32, generation: u64) {
        self.dmabuf
            .failed_surface_generations
            .insert(surface_id, generation);
    }

    #[cfg(test)]
    pub(super) fn test_failed_surface_generation(&self, surface_id: u32) -> Option<u64> {
        self.dmabuf
            .failed_surface_generations
            .get(&surface_id)
            .copied()
    }

    #[cfg(test)]
    pub(super) fn test_insert_dmabuf_registry_texture(
        &mut self,
        key: oblivion_one::render_backend::buffer::DmabufImageKey,
        texture: glow::Texture,
        buffer_lifetime: oblivion_one::render_backend::buffer::WeakBufferIdentity,
    ) {
        self.dmabuf
            .test_insert_texture(key, texture, buffer_lifetime);
    }

    #[cfg(test)]
    pub(super) fn test_dmabuf_registry_entry_count(&self) -> usize {
        self.dmabuf.registry_entry_count()
    }

    #[cfg(test)]
    pub(super) fn test_surface_resource_identity(&self, surface_id: u32) -> Option<usize> {
        self.surfaces
            .resources
            .get(&surface_id)
            .map(|resource| std::ptr::from_ref(resource) as usize)
    }
}

pub(super) struct RendererResourceCaptureSnapshot {
    failed_surface_generations: HashMap<u32, u64>,
}

pub(super) struct ResourceTelemetry<'a> {
    frame_stats: &'a mut GlesSceneFrameStats,
}

impl<'a> ResourceTelemetry<'a> {
    pub(super) fn new(frame_stats: &'a mut GlesSceneFrameStats) -> Self {
        Self { frame_stats }
    }

    pub(super) fn shm_upload_bytes(&mut self, bytes: usize) {
        self.frame_stats.shm_upload_bytes = self.frame_stats.shm_upload_bytes.saturating_add(bytes);
    }

    pub(super) fn shm_full_resync(&mut self) {
        self.frame_stats.shm_full_resyncs = self.frame_stats.shm_full_resyncs.saturating_add(1);
    }

    pub(super) fn dmabuf_imported(&mut self) {
        self.frame_stats.dmabuf_imports = self.frame_stats.dmabuf_imports.saturating_add(1);
    }

    pub(super) fn dmabuf_current_resource_reuse(&mut self) {
        self.frame_stats.dmabuf_current_resource_reuses = self
            .frame_stats
            .dmabuf_current_resource_reuses
            .saturating_add(1);
    }

    pub(super) fn dmabuf_reuse(&mut self) {
        self.frame_stats.dmabuf_reuses = self.frame_stats.dmabuf_reuses.saturating_add(1);
    }

    pub(super) fn dmabuf_import_failure(&mut self) {
        self.frame_stats.dmabuf_import_failures =
            self.frame_stats.dmabuf_import_failures.saturating_add(1);
    }

    pub(super) fn dmabuf_registry_hit(&mut self) {
        self.frame_stats.dmabuf_registry_hits =
            self.frame_stats.dmabuf_registry_hits.saturating_add(1);
    }

    pub(super) fn dmabuf_registry_miss(&mut self) {
        self.frame_stats.dmabuf_registry_misses =
            self.frame_stats.dmabuf_registry_misses.saturating_add(1);
    }

    pub(super) fn dmabuf_registry_insertion(&mut self) {
        self.frame_stats.dmabuf_registry_insertions = self
            .frame_stats
            .dmabuf_registry_insertions
            .saturating_add(1);
    }

    pub(super) fn dmabuf_registry_dead_lifetime_eviction(&mut self) {
        self.frame_stats.dmabuf_registry_dead_lifetime_evictions = self
            .frame_stats
            .dmabuf_registry_dead_lifetime_evictions
            .saturating_add(1);
    }

    pub(super) fn publish_dmabuf_registry_metrics(
        &mut self,
        active_surface_bindings: usize,
        registry_entries: usize,
        resident_known_logical_bytes: u64,
    ) {
        self.frame_stats.dmabuf_active_surface_bindings = active_surface_bindings;
        self.frame_stats.dmabuf_registry_entries = registry_entries;
        self.frame_stats.dmabuf_registry_known_logical_bytes = resident_known_logical_bytes;
    }

    pub(super) fn publish_client_import_residency(
        &mut self,
        summary: crate::gpu_residency::GpuResidencyDomainSnapshot,
    ) {
        self.frame_stats.gpu_residency.client_import = summary;
        self.frame_stats
            .gpu_residency
            .client_import_observation_available = true;
    }

    pub(super) fn publish_renderer_dmabuf_registry_diagnostics(
        &mut self,
        diagnostics: crate::gpu_residency::RendererDmabufRegistryDiagnostics,
    ) {
        self.frame_stats.gpu_residency.renderer_dmabuf_registry = diagnostics;
        self.frame_stats
            .gpu_residency
            .client_import_observation_available = true;
    }
}
