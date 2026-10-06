use std::collections::HashMap;

use khronos_egl as egl;
use oblivion_one::render_backend::buffer::{DmabufImageKey, WeakBufferIdentity};

use super::super::{EglInstance, native_egl_debug_enabled};
use super::ResourceTelemetry;
use super::dmabuf_import::{
    DmabufImportDiagnosticContext, DmabufImporter, settle_dmabuf_import_result,
};
use super::image::{EglImageResource, destroy_image_resource};

pub(super) const MAX_CACHED_DMABUF_RESOURCES_PER_SURFACE: usize = 4;

pub(super) struct CachedDmabufResource<R> {
    pub(super) image: R,
    pub(super) buffer_lifetime: WeakBufferIdentity,
    pub(super) surface_id: u32,
}

pub(super) struct DmabufResourceState {
    cache: HashMap<DmabufImageKey, CachedDmabufResource<EglImageResource>>,
    pub(super) cache_peak_entries: usize,
    pub(super) cache_max_entries_for_one_surface: usize,
    pub(super) failed_surface_generations: HashMap<u32, u64>,
    importer: DmabufImporter,
}

impl DmabufResourceState {
    pub(super) fn new(
        image_target_texture_2d: Option<super::super::GlEglImageTargetTexture2DOes>,
    ) -> Self {
        Self {
            cache: HashMap::new(),
            cache_peak_entries: 0,
            cache_max_entries_for_one_surface: 0,
            failed_surface_generations: HashMap::new(),
            importer: DmabufImporter::new(image_target_texture_2d),
        }
    }

    pub(super) fn importer(&self) -> &DmabufImporter {
        &self.importer
    }

    pub(super) fn settle_import_result<T>(
        &mut self,
        result: super::super::RendererResult<T>,
        context: DmabufImportDiagnosticContext,
        telemetry: &mut ResourceTelemetry<'_>,
    ) -> super::super::RendererResult<Option<T>> {
        settle_dmabuf_import_result(
            result,
            context,
            telemetry,
            &mut self.failed_surface_generations,
        )
    }

    pub(super) fn take_cached(
        &mut self,
        key: &DmabufImageKey,
    ) -> Option<CachedDmabufResource<EglImageResource>> {
        self.cache.remove(key)
    }

    pub(super) fn note_cache_hit(&self, telemetry: &mut ResourceTelemetry<'_>) {
        telemetry.dmabuf_cache_hit();
    }

    pub(super) fn note_cache_miss(&self, telemetry: &mut ResourceTelemetry<'_>) {
        telemetry.dmabuf_cache_miss();
    }

    pub(super) fn cache_or_destroy_resource(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
        surface_id: u32,
        key: Option<DmabufImageKey>,
        buffer_lifetime: Option<WeakBufferIdentity>,
        image: EglImageResource,
        telemetry: &mut ResourceTelemetry<'_>,
    ) {
        let Some(key) = key else {
            destroy_image_resource(gl, egl, egl_display, image);
            return;
        };
        let Some(buffer_lifetime) = buffer_lifetime else {
            destroy_image_resource(gl, egl, egl_display, image);
            return;
        };
        if !buffer_lifetime.is_alive() {
            if native_egl_debug_enabled() {
                eprintln!(
                    "oblivion-one compositor: dmabuf cache=evict reason=dead-before-cache key={key:?}"
                );
            }
            destroy_image_resource(gl, egl, egl_display, image);
            telemetry.dmabuf_cache_evicted_dead();
            return;
        }

        self.prune_for_surface(gl, egl, egl_display, surface_id, telemetry);
        if let Some(replaced) = self.cache.insert(
            key,
            CachedDmabufResource {
                image,
                buffer_lifetime,
                surface_id,
            },
        ) {
            destroy_image_resource(gl, egl, egl_display, replaced.image);
        }
        telemetry.dmabuf_cache_insertion();
        let surface_entries = self
            .cache
            .values()
            .filter(|cached| cached.surface_id == surface_id)
            .count();
        self.cache_max_entries_for_one_surface =
            self.cache_max_entries_for_one_surface.max(surface_entries);
        self.cache_peak_entries = self.cache_peak_entries.max(self.cache.len());
    }

    fn prune_for_surface(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
        surface_id: u32,
        telemetry: &mut ResourceTelemetry<'_>,
    ) {
        let cached = self
            .cache
            .values()
            .filter(|cached| cached.surface_id == surface_id)
            .count();
        if cached < MAX_CACHED_DMABUF_RESOURCES_PER_SURFACE {
            return;
        }
        let Some(key) = self
            .cache
            .iter()
            .find_map(|(key, cached)| (cached.surface_id == surface_id).then_some(key.clone()))
        else {
            return;
        };
        if let Some(resource) = self.cache.remove(&key) {
            if native_egl_debug_enabled() {
                eprintln!(
                    "oblivion-one compositor: dmabuf cache=evict reason=surface-bound key={key:?}"
                );
            }
            destroy_image_resource(gl, egl, egl_display, resource.image);
            telemetry.dmabuf_cache_evicted_surface_bound();
        }
    }

    pub(super) fn destroy_cached_for_surface(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
        surface_id: u32,
        telemetry: &mut ResourceTelemetry<'_>,
    ) {
        let keys = self
            .cache
            .iter()
            .filter_map(|(key, cached)| (cached.surface_id == surface_id).then_some(key.clone()))
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(resource) = self.cache.remove(&key) {
                if native_egl_debug_enabled() {
                    eprintln!(
                        "oblivion-one compositor: dmabuf cache=evict reason=surface-destroyed key={key:?}"
                    );
                }
                destroy_image_resource(gl, egl, egl_display, resource.image);
                telemetry.dmabuf_cache_evicted_surface_destroyed();
            }
        }
    }

    pub(super) fn evict_dead(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
        telemetry: &mut ResourceTelemetry<'_>,
    ) {
        let dead = dead_cached_dmabuf_keys(&self.cache);
        for key in dead {
            if let Some(cached) = self.cache.remove(&key) {
                if native_egl_debug_enabled() {
                    eprintln!(
                        "oblivion-one compositor: dmabuf cache=evict reason=buffer-dead key={key:?}"
                    );
                }
                destroy_image_resource(gl, egl, egl_display, cached.image);
                telemetry.dmabuf_cache_evicted_dead();
            }
        }
    }

    #[cfg(test)]
    pub(super) fn cache_entry_count(&self) -> usize {
        self.cache.len()
    }

    #[cfg(test)]
    pub(super) fn test_insert_texture(
        &mut self,
        key: DmabufImageKey,
        texture: glow::Texture,
        buffer_lifetime: WeakBufferIdentity,
        surface_id: u32,
    ) {
        self.cache.insert(
            key,
            CachedDmabufResource {
                image: EglImageResource {
                    texture,
                    size: (1, 1),
                    generation: 1,
                    egl_image: None,
                },
                buffer_lifetime,
                surface_id,
            },
        );
    }

    pub(super) fn publish_metrics(&self, telemetry: &mut ResourceTelemetry<'_>) {
        telemetry.publish_dmabuf_cache_metrics(
            self.cache.len(),
            self.cache_peak_entries,
            self.cache_max_entries_for_one_surface,
        );
    }

    pub(super) fn destroy_all(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
    ) {
        for (_, resource) in self.cache.drain() {
            destroy_image_resource(gl, egl, egl_display, resource.image);
        }
    }
}

pub(super) fn dead_cached_dmabuf_keys<R>(
    cache: &HashMap<DmabufImageKey, CachedDmabufResource<R>>,
) -> Vec<DmabufImageKey> {
    cache
        .iter()
        .filter_map(|(key, cached)| (!cached.buffer_lifetime.is_alive()).then_some(key.clone()))
        .collect()
}
