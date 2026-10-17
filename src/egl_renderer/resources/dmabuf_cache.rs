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
    cache_logical_bytes: u64,
    cache_unknown_size_entries: usize,
    active_mapping_count: usize,
    active_known_logical_bytes: u64,
    active_unknown_size_count: usize,
    pub(super) cache_peak_entries: usize,
    cache_peak_known_logical_bytes: u64,
    pub(super) cache_max_entries_for_one_surface: usize,
    cache_hits_total: u64,
    cache_misses_total: u64,
    cache_insertions_total: u64,
    cache_replacements_total: u64,
    cache_evictions_dead_total: u64,
    cache_evictions_surface_bound_total: u64,
    cache_evictions_surface_destroyed_total: u64,
    imports_total: u64,
    current_resource_reuses_total: u64,
    cache_reuses_total: u64,
    import_failures_total: u64,
    pub(super) failed_surface_generations: HashMap<u32, u64>,
    importer: DmabufImporter,
}

impl DmabufResourceState {
    pub(super) fn new(
        image_target_texture_2d: Option<super::super::GlEglImageTargetTexture2DOes>,
    ) -> Self {
        Self {
            cache: HashMap::new(),
            cache_logical_bytes: 0,
            cache_unknown_size_entries: 0,
            active_mapping_count: 0,
            active_known_logical_bytes: 0,
            active_unknown_size_count: 0,
            cache_peak_entries: 0,
            cache_peak_known_logical_bytes: 0,
            cache_max_entries_for_one_surface: 0,
            cache_hits_total: 0,
            cache_misses_total: 0,
            cache_insertions_total: 0,
            cache_replacements_total: 0,
            cache_evictions_dead_total: 0,
            cache_evictions_surface_bound_total: 0,
            cache_evictions_surface_destroyed_total: 0,
            imports_total: 0,
            current_resource_reuses_total: 0,
            cache_reuses_total: 0,
            import_failures_total: 0,
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
        if result.is_err() {
            self.note_import_failure();
        }
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
        self.remove_cached_resource(key)
    }

    pub(super) fn note_cache_hit(&mut self, telemetry: &mut ResourceTelemetry<'_>) {
        self.cache_hits_total = self.cache_hits_total.saturating_add(1);
        self.cache_reuses_total = self.cache_reuses_total.saturating_add(1);
        telemetry.dmabuf_cache_hit();
        telemetry.dmabuf_reuse();
    }

    pub(super) fn note_cache_miss(&mut self, telemetry: &mut ResourceTelemetry<'_>) {
        self.cache_misses_total = self.cache_misses_total.saturating_add(1);
        telemetry.dmabuf_cache_miss();
    }

    pub(super) fn note_import_success(&mut self, telemetry: &mut ResourceTelemetry<'_>) {
        self.imports_total = self.imports_total.saturating_add(1);
        telemetry.dmabuf_imported();
    }

    pub(super) fn note_current_resource_reuse(&mut self, telemetry: &mut ResourceTelemetry<'_>) {
        self.current_resource_reuses_total = self.current_resource_reuses_total.saturating_add(1);
        telemetry.dmabuf_current_resource_reuse();
        telemetry.dmabuf_reuse();
    }

    fn note_import_failure(&mut self) {
        self.import_failures_total = self.import_failures_total.saturating_add(1);
    }

    fn note_cache_inserted(&mut self, telemetry: Option<&mut ResourceTelemetry<'_>>) {
        self.cache_insertions_total = self.cache_insertions_total.saturating_add(1);
        if let Some(telemetry) = telemetry {
            telemetry.dmabuf_cache_insertion();
        }
    }

    fn note_cache_evicted_dead(&mut self, telemetry: &mut ResourceTelemetry<'_>) {
        self.cache_evictions_dead_total = self.cache_evictions_dead_total.saturating_add(1);
        telemetry.dmabuf_cache_evicted_dead();
    }

    fn note_cache_evicted_surface_bound(&mut self, telemetry: &mut ResourceTelemetry<'_>) {
        self.cache_evictions_surface_bound_total =
            self.cache_evictions_surface_bound_total.saturating_add(1);
        telemetry.dmabuf_cache_evicted_surface_bound();
    }

    fn note_cache_evicted_surface_destroyed(&mut self, telemetry: &mut ResourceTelemetry<'_>) {
        self.cache_evictions_surface_destroyed_total = self
            .cache_evictions_surface_destroyed_total
            .saturating_add(1);
        telemetry.dmabuf_cache_evicted_surface_destroyed();
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
        let logical_bytes = key.logical_payload_bytes();
        if let Some(replaced) = self.remove_cached_resource(&key) {
            self.cache_replacements_total = self.cache_replacements_total.saturating_add(1);
            destroy_image_resource(gl, egl, egl_display, replaced.image);
        }
        self.cache.insert(
            key,
            CachedDmabufResource {
                image,
                buffer_lifetime,
                surface_id,
            },
        );
        self.note_cache_add(logical_bytes);
        self.note_cache_inserted(Some(telemetry));
        let surface_entries = self
            .cache
            .values()
            .filter(|cached| cached.surface_id == surface_id)
            .count();
        self.cache_max_entries_for_one_surface =
            self.cache_max_entries_for_one_surface.max(surface_entries);
        self.cache_peak_entries = self.cache_peak_entries.max(self.cache.len());
        self.cache_peak_known_logical_bytes = self
            .cache_peak_known_logical_bytes
            .max(self.cache_logical_bytes);
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
        if let Some(resource) = self.remove_cached_resource(&key) {
            if native_egl_debug_enabled() {
                eprintln!(
                    "oblivion-one compositor: dmabuf cache=evict reason=surface-bound key={key:?}"
                );
            }
            destroy_image_resource(gl, egl, egl_display, resource.image);
            self.note_cache_evicted_surface_bound(telemetry);
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
            if let Some(resource) = self.remove_cached_resource(&key) {
                if native_egl_debug_enabled() {
                    eprintln!(
                        "oblivion-one compositor: dmabuf cache=evict reason=surface-destroyed key={key:?}"
                    );
                }
                destroy_image_resource(gl, egl, egl_display, resource.image);
                self.note_cache_evicted_surface_destroyed(telemetry);
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
            if let Some(cached) = self.remove_cached_resource(&key) {
                if native_egl_debug_enabled() {
                    eprintln!(
                        "oblivion-one compositor: dmabuf cache=evict reason=buffer-dead key={key:?}"
                    );
                }
                destroy_image_resource(gl, egl, egl_display, cached.image);
                self.note_cache_evicted_dead(telemetry);
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
        if self.remove_cached_resource(&key).is_some() {
            self.cache_replacements_total = self.cache_replacements_total.saturating_add(1);
        }
        self.note_cache_add(key.logical_payload_bytes());
        self.cache.insert(
            key,
            CachedDmabufResource {
                image: EglImageResource {
                    texture,
                    size: (1, 1),
                    generation: 1,
                    egl_image: None,
                    telemetry: super::image::EglImageResourceTelemetry::Untracked,
                },
                buffer_lifetime,
                surface_id,
            },
        );
        self.note_cache_inserted(None);
        self.cache_peak_entries = self.cache_peak_entries.max(self.cache.len());
        self.cache_peak_known_logical_bytes = self
            .cache_peak_known_logical_bytes
            .max(self.cache_logical_bytes);
        let surface_entries = self
            .cache
            .values()
            .filter(|cached| cached.surface_id == surface_id)
            .count();
        self.cache_max_entries_for_one_surface =
            self.cache_max_entries_for_one_surface.max(surface_entries);
    }

    pub(super) fn publish_active_metrics(
        &mut self,
        active_mapping_count: usize,
        active_known_logical_bytes: u64,
        active_unknown_size_count: usize,
    ) {
        self.active_mapping_count = active_mapping_count;
        self.active_known_logical_bytes = active_known_logical_bytes;
        self.active_unknown_size_count = active_unknown_size_count;
    }

    pub(super) fn publish_metrics(&self, telemetry: &mut ResourceTelemetry<'_>) {
        telemetry.publish_dmabuf_cache_metrics(
            self.cache.len(),
            self.cache_peak_entries,
            self.cache_max_entries_for_one_surface,
        );
        telemetry.publish_renderer_import_cache_diagnostics(self.diagnostics());
        let mut residency = crate::gpu_residency::GpuResidencyDomainSnapshot::new(
            crate::gpu_residency::GpuResidencyOwner::EglRenderer,
            crate::gpu_residency::ResourceDomain::ClientImport,
            crate::gpu_residency::ResourceOwnership::ClientImportedExternallyBacked,
            crate::gpu_residency::ResourceCoverage::Instrumented,
        );
        residency.record_resources(
            crate::gpu_residency::ResourceResidency::Active,
            self.active_mapping_count
                .saturating_sub(self.active_unknown_size_count) as u64,
            Some(self.active_known_logical_bytes),
            true,
            true,
        );
        residency.record_resources(
            crate::gpu_residency::ResourceResidency::Active,
            self.active_unknown_size_count as u64,
            None,
            true,
            true,
        );
        residency.record_resources(
            crate::gpu_residency::ResourceResidency::WarmReusable,
            self.cache
                .len()
                .saturating_sub(self.cache_unknown_size_entries) as u64,
            Some(self.cache_logical_bytes),
            true,
            true,
        );
        residency.record_resources(
            crate::gpu_residency::ResourceResidency::WarmReusable,
            self.cache_unknown_size_entries as u64,
            None,
            true,
            true,
        );
        telemetry.publish_client_import_residency(residency);
    }

    fn diagnostics(&self) -> crate::gpu_residency::RendererImportCacheDiagnostics {
        crate::gpu_residency::RendererImportCacheDiagnostics {
            active_mapping_count: self.active_mapping_count as u64,
            active_known_logical_bytes: self.active_known_logical_bytes,
            active_unknown_size_count: self.active_unknown_size_count as u64,
            cached_mapping_count: self.cache.len() as u64,
            cached_known_logical_bytes: self.cache_logical_bytes,
            cached_unknown_size_count: self.cache_unknown_size_entries as u64,
            cache_peak_entries: self.cache_peak_entries as u64,
            cache_peak_known_logical_bytes: self.cache_peak_known_logical_bytes,
            cache_hits_total: self.cache_hits_total,
            cache_misses_total: self.cache_misses_total,
            cache_insertions_total: self.cache_insertions_total,
            cache_replacements_total: self.cache_replacements_total,
            cache_evictions_dead_total: self.cache_evictions_dead_total,
            cache_evictions_surface_bound_total: self.cache_evictions_surface_bound_total,
            cache_evictions_surface_destroyed_total: self.cache_evictions_surface_destroyed_total,
            imports_total: self.imports_total,
            current_resource_reuses_total: self.current_resource_reuses_total,
            cache_reuses_total: self.cache_reuses_total,
            import_failures_total: self.import_failures_total,
            max_entries_for_one_surface: self.cache_max_entries_for_one_surface as u64,
            per_surface_entry_limit: MAX_CACHED_DMABUF_RESOURCES_PER_SURFACE as u64,
        }
    }

    pub(super) fn destroy_all(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
    ) {
        let resources = std::mem::take(&mut self.cache);
        self.cache_logical_bytes = 0;
        self.cache_unknown_size_entries = 0;
        for (_, resource) in resources {
            destroy_image_resource(gl, egl, egl_display, resource.image);
        }
    }

    fn remove_cached_resource(
        &mut self,
        key: &DmabufImageKey,
    ) -> Option<CachedDmabufResource<EglImageResource>> {
        let resource = self.cache.remove(key)?;
        if let Some(bytes) = key.logical_payload_bytes() {
            self.cache_logical_bytes = self.cache_logical_bytes.saturating_sub(bytes);
        } else {
            self.cache_unknown_size_entries = self.cache_unknown_size_entries.saturating_sub(1);
        }
        Some(resource)
    }

    fn note_cache_add(&mut self, logical_bytes: Option<u64>) {
        if let Some(bytes) = logical_bytes {
            self.cache_logical_bytes = self.cache_logical_bytes.saturating_add(bytes);
        } else {
            self.cache_unknown_size_entries = self.cache_unknown_size_entries.saturating_add(1);
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

#[cfg(test)]
mod tests {
    use super::*;
    use oblivion_one::render_backend::buffer::{
        BufferIdAllocator, BufferSize, DmabufPlane, DmabufPlaneDescriptor, DrmFormat, DrmModifier,
    };

    fn test_identity_and_key(
        ids: &mut BufferIdAllocator,
        format: DrmFormat,
    ) -> (
        oblivion_one::render_backend::buffer::BufferIdentity,
        DmabufImageKey,
    ) {
        let identity = ids.allocate().expect("test buffer identity");
        let fd = std::fs::File::open("/dev/null")
            .expect("/dev/null exists")
            .into();
        let handle = oblivion_one::render_backend::buffer::DmabufBufferHandle::new(
            BufferSize::new(2, 2).expect("test size"),
            format,
            vec![DmabufPlane::new(
                fd,
                DmabufPlaneDescriptor {
                    plane_index: 0,
                    offset: 0,
                    stride: 8,
                    modifier: DrmModifier::LINEAR,
                },
            )],
        )
        .expect("valid test dma-buf");
        let key = DmabufImageKey::from_handle(identity.id(), &handle);
        (identity, key)
    }

    #[test]
    fn renderer_cache_diagnostics_keep_active_cached_and_cumulative_values_separate() {
        let mut state = DmabufResourceState::new(None);
        state.publish_active_metrics(3, 8192, 1);
        let mut ids = BufferIdAllocator::default();
        let (known_identity, known_key) = test_identity_and_key(&mut ids, DrmFormat::Xrgb8888);
        let (unknown_identity, unknown_key) =
            test_identity_and_key(&mut ids, DrmFormat::Other(u32::from_le_bytes(*b"NV12")));
        state.test_insert_texture(
            known_key,
            glow::NativeTexture(std::num::NonZeroU32::new(1).unwrap()),
            known_identity.downgrade(),
            1,
        );
        state.test_insert_texture(
            unknown_key,
            glow::NativeTexture(std::num::NonZeroU32::new(2).unwrap()),
            unknown_identity.downgrade(),
            1,
        );
        let mut frame_stats = super::super::GlesSceneFrameStats::default();
        let mut telemetry = ResourceTelemetry::new(&mut frame_stats);
        for _ in 0..7 {
            state.note_cache_hit(&mut telemetry);
        }
        for _ in 0..9 {
            state.note_cache_miss(&mut telemetry);
        }
        for _ in 0..6 {
            state.note_import_success(&mut telemetry);
        }
        for _ in 0..10 {
            state.note_current_resource_reuse(&mut telemetry);
        }
        state.note_import_failure();
        state.note_import_failure();
        state.note_cache_evicted_dead(&mut telemetry);
        state.note_cache_evicted_surface_bound(&mut telemetry);
        state.note_cache_evicted_surface_destroyed(&mut telemetry);

        let diagnostics = state.diagnostics();
        assert_eq!(diagnostics.active_mapping_count, 3);
        assert_eq!(diagnostics.active_known_logical_bytes, 8192);
        assert_eq!(diagnostics.active_unknown_size_count, 1);
        assert_eq!(diagnostics.cached_mapping_count, 2);
        assert_eq!(diagnostics.cached_known_logical_bytes, 16);
        assert_eq!(diagnostics.cached_unknown_size_count, 1);
        assert_eq!(diagnostics.cache_peak_entries, 2);
        assert_eq!(diagnostics.cache_peak_known_logical_bytes, 16);
        assert_eq!(diagnostics.cache_hits_total, 7);
        assert_eq!(diagnostics.cache_misses_total, 9);
        assert_eq!(diagnostics.cache_insertions_total, 2);
        assert_eq!(diagnostics.cache_evictions_dead_total, 1);
        assert_eq!(diagnostics.imports_total, 6);
        assert_eq!(diagnostics.current_resource_reuses_total, 10);
        assert_eq!(diagnostics.cache_reuses_total, 7);
        assert_eq!(diagnostics.import_failures_total, 2);
        assert_eq!(diagnostics.max_entries_for_one_surface, 2);
        assert_eq!(diagnostics.per_surface_entry_limit, 4);
    }
}
