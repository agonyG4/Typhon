use std::collections::{HashMap, HashSet};

use khronos_egl as egl;
use oblivion_one::render_backend::buffer::{DmabufImageKey, WeakBufferIdentity};

use super::super::{EglInstance, native_egl_debug_enabled};
use super::ResourceTelemetry;
use super::dmabuf_import::{
    DmabufImportDiagnosticContext, DmabufImporter, settle_dmabuf_import_result,
};
use super::image::{EglImageResource, destroy_image_resource};

pub(super) struct DmabufRegistryEntry<R> {
    pub(super) image: R,
    pub(super) buffer_lifetime: WeakBufferIdentity,
}

/// Renderer-wide owner of imported mappings. Each key carries exact import
/// metadata and each entry keeps only a weak buffer-lifetime signal.
pub(super) struct DmabufImportRegistry<R> {
    entries: HashMap<DmabufImageKey, DmabufRegistryEntry<R>>,
}

impl<R> Default for DmabufImportRegistry<R> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

impl<R> DmabufImportRegistry<R> {
    pub(super) fn contains_key(&self, key: &DmabufImageKey) -> bool {
        self.entries.contains_key(key)
    }

    pub(super) fn get(&self, key: &DmabufImageKey) -> Option<&DmabufRegistryEntry<R>> {
        self.entries
            .get(key)
            .filter(|entry| entry.buffer_lifetime.is_alive())
    }

    pub(super) fn insert(
        &mut self,
        key: DmabufImageKey,
        image: R,
        buffer_lifetime: WeakBufferIdentity,
    ) {
        assert_eq!(
            buffer_lifetime.id(),
            key.buffer_id(),
            "registry key and weak buffer lifetime must share the same BufferId"
        );
        assert!(
            !self.entries.contains_key(&key),
            "a DMA-BUF registry key cannot be replaced before its old mapping is retired"
        );
        let _ = self.entries.insert(
            key,
            DmabufRegistryEntry {
                image,
                buffer_lifetime,
            },
        );
    }

    #[cfg(test)]
    pub(super) fn remove(&mut self, key: &DmabufImageKey) -> Option<DmabufRegistryEntry<R>> {
        self.entries.remove(key)
    }

    pub(super) fn remove_dead(&mut self) -> Vec<(DmabufImageKey, DmabufRegistryEntry<R>)> {
        let dead_keys = self
            .entries
            .iter()
            .filter_map(|(key, entry)| (!entry.buffer_lifetime.is_alive()).then_some(key.clone()))
            .collect::<Vec<_>>();
        dead_keys
            .into_iter()
            .filter_map(|key| self.entries.remove(&key).map(|entry| (key, entry)))
            .collect()
    }

    pub(super) fn drain(&mut self) -> Vec<(DmabufImageKey, DmabufRegistryEntry<R>)> {
        self.entries.drain().collect()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (&DmabufImageKey, &DmabufRegistryEntry<R>)> {
        self.entries.iter()
    }

    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The single renderer owner for DMA-BUF imports, their lifecycle counters,
/// and deferred import-failure state.
pub(super) struct DmabufResourceState {
    registry: DmabufImportRegistry<EglImageResource>,
    resident_known_logical_bytes: u64,
    resident_unknown_size_count: usize,
    active_surface_binding_count: usize,
    active_registry_mapping_count: usize,
    active_registry_known_logical_bytes: u64,
    active_registry_unknown_size_count: usize,
    warm_registry_mapping_count: usize,
    warm_registry_known_logical_bytes: u64,
    warm_registry_unknown_size_count: usize,
    registry_peak_mapping_count: usize,
    registry_peak_known_logical_bytes: u64,
    registry_hits_total: u64,
    registry_misses_total: u64,
    registry_insertions_total: u64,
    registry_retirements_total: u64,
    dead_lifetime_evictions_total: u64,
    imports_total: u64,
    current_resource_reuses_total: u64,
    import_failures_total: u64,
    pub(super) failed_surface_generations: HashMap<u32, u64>,
    importer: DmabufImporter,
}

impl DmabufResourceState {
    pub(super) fn new(
        image_target_texture_2d: Option<super::super::GlEglImageTargetTexture2DOes>,
    ) -> Self {
        Self {
            registry: DmabufImportRegistry::default(),
            resident_known_logical_bytes: 0,
            resident_unknown_size_count: 0,
            active_surface_binding_count: 0,
            active_registry_mapping_count: 0,
            active_registry_known_logical_bytes: 0,
            active_registry_unknown_size_count: 0,
            warm_registry_mapping_count: 0,
            warm_registry_known_logical_bytes: 0,
            warm_registry_unknown_size_count: 0,
            registry_peak_mapping_count: 0,
            registry_peak_known_logical_bytes: 0,
            registry_hits_total: 0,
            registry_misses_total: 0,
            registry_insertions_total: 0,
            registry_retirements_total: 0,
            dead_lifetime_evictions_total: 0,
            imports_total: 0,
            current_resource_reuses_total: 0,
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
            self.import_failures_total = self.import_failures_total.saturating_add(1);
        }
        settle_dmabuf_import_result(
            result,
            context,
            telemetry,
            &mut self.failed_surface_generations,
        )
    }

    pub(super) fn lookup_texture(
        &mut self,
        key: &DmabufImageKey,
        telemetry: &mut ResourceTelemetry<'_>,
    ) -> Option<glow::Texture> {
        match self.registry.get(key).map(|entry| entry.image.texture) {
            Some(texture) => {
                self.note_registry_hit(telemetry);
                Some(texture)
            }
            None => {
                self.note_registry_miss(telemetry);
                None
            }
        }
    }

    pub(super) fn contains_live_key(&self, key: &DmabufImageKey) -> bool {
        self.registry.get(key).is_some()
    }

    pub(super) fn texture_for_key(&self, key: &DmabufImageKey) -> Option<glow::Texture> {
        self.registry.get(key).map(|entry| entry.image.texture)
    }

    fn note_registry_hit(&mut self, telemetry: &mut ResourceTelemetry<'_>) {
        self.registry_hits_total = self.registry_hits_total.saturating_add(1);
        telemetry.dmabuf_registry_hit();
        telemetry.dmabuf_reuse();
    }

    fn note_registry_miss(&mut self, telemetry: &mut ResourceTelemetry<'_>) {
        self.registry_misses_total = self.registry_misses_total.saturating_add(1);
        telemetry.dmabuf_registry_miss();
    }

    pub(super) fn note_import_success(&mut self, telemetry: &mut ResourceTelemetry<'_>) {
        self.imports_total = self.imports_total.saturating_add(1);
        telemetry.dmabuf_imported();
    }

    pub(super) fn insert_import(
        &mut self,
        key: DmabufImageKey,
        buffer_lifetime: WeakBufferIdentity,
        image: EglImageResource,
        telemetry: &mut ResourceTelemetry<'_>,
    ) {
        assert!(
            !self.registry.contains_key(&key),
            "a DMA-BUF registry mapping must be reused or retired before replacement"
        );
        self.insert_registry_entry(key, buffer_lifetime, image, Some(telemetry));
    }

    pub(super) fn note_current_resource_reuse(&mut self, telemetry: &mut ResourceTelemetry<'_>) {
        self.current_resource_reuses_total = self.current_resource_reuses_total.saturating_add(1);
        telemetry.dmabuf_current_resource_reuse();
        telemetry.dmabuf_reuse();
    }

    fn insert_registry_entry(
        &mut self,
        key: DmabufImageKey,
        buffer_lifetime: WeakBufferIdentity,
        image: EglImageResource,
        telemetry: Option<&mut ResourceTelemetry<'_>>,
    ) {
        self.registry.insert(key.clone(), image, buffer_lifetime);
        let Some(logical_bytes) = key.logical_payload_bytes() else {
            self.resident_unknown_size_count = self.resident_unknown_size_count.saturating_add(1);
            self.registry_peak_mapping_count =
                self.registry_peak_mapping_count.max(self.registry.len());
            self.note_registry_insertion(telemetry);
            return;
        };
        self.resident_known_logical_bytes = self
            .resident_known_logical_bytes
            .saturating_add(logical_bytes);
        self.registry_peak_mapping_count =
            self.registry_peak_mapping_count.max(self.registry.len());
        self.registry_peak_known_logical_bytes = self
            .registry_peak_known_logical_bytes
            .max(self.resident_known_logical_bytes);
        self.note_registry_insertion(telemetry);
    }

    fn note_registry_insertion(&mut self, telemetry: Option<&mut ResourceTelemetry<'_>>) {
        self.registry_insertions_total = self.registry_insertions_total.saturating_add(1);
        if let Some(telemetry) = telemetry {
            telemetry.dmabuf_registry_insertion();
        }
    }

    pub(super) fn evict_dead(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
        telemetry: &mut ResourceTelemetry<'_>,
    ) {
        for (key, resource) in self.remove_dead_entries() {
            if native_egl_debug_enabled() {
                eprintln!(
                    "oblivion-one compositor: dmabuf registry=evict reason=buffer-lifetime-ended key={key:?}"
                );
            }
            destroy_image_resource(gl, egl, egl_display, resource.image);
            telemetry.dmabuf_registry_dead_lifetime_eviction();
        }
    }

    fn remove_dead_entries(
        &mut self,
    ) -> Vec<(DmabufImageKey, DmabufRegistryEntry<EglImageResource>)> {
        let dead = self.registry.remove_dead();
        for (key, _) in &dead {
            self.note_registry_removal(key);
            self.dead_lifetime_evictions_total =
                self.dead_lifetime_evictions_total.saturating_add(1);
            self.registry_retirements_total = self.registry_retirements_total.saturating_add(1);
        }
        dead
    }

    pub(super) fn publish_metrics(
        &mut self,
        active_surface_binding_count: usize,
        active_keys: &HashSet<DmabufImageKey>,
        telemetry: &mut ResourceTelemetry<'_>,
    ) {
        self.active_surface_binding_count = active_surface_binding_count;
        self.active_registry_mapping_count = 0;
        self.active_registry_known_logical_bytes = 0;
        self.active_registry_unknown_size_count = 0;
        self.warm_registry_mapping_count = 0;
        self.warm_registry_known_logical_bytes = 0;
        self.warm_registry_unknown_size_count = 0;

        for (key, _) in self.registry.iter() {
            let (mapping_count, known_bytes, unknown_count) = if active_keys.contains(key) {
                (
                    &mut self.active_registry_mapping_count,
                    &mut self.active_registry_known_logical_bytes,
                    &mut self.active_registry_unknown_size_count,
                )
            } else {
                (
                    &mut self.warm_registry_mapping_count,
                    &mut self.warm_registry_known_logical_bytes,
                    &mut self.warm_registry_unknown_size_count,
                )
            };
            *mapping_count = mapping_count.saturating_add(1);
            if let Some(bytes) = key.logical_payload_bytes() {
                *known_bytes = known_bytes.saturating_add(bytes);
            } else {
                *unknown_count = unknown_count.saturating_add(1);
            }
        }

        telemetry.publish_dmabuf_registry_metrics(
            self.active_surface_binding_count,
            self.registry.len(),
            self.resident_known_logical_bytes,
        );
        telemetry.publish_renderer_dmabuf_registry_diagnostics(self.diagnostics());

        let mut residency = crate::gpu_residency::GpuResidencyDomainSnapshot::new(
            crate::gpu_residency::GpuResidencyOwner::EglRenderer,
            crate::gpu_residency::ResourceDomain::ClientImport,
            crate::gpu_residency::ResourceOwnership::ClientImportedExternallyBacked,
            crate::gpu_residency::ResourceCoverage::Instrumented,
        );
        residency.record_resources(
            crate::gpu_residency::ResourceResidency::Active,
            self.active_registry_mapping_count
                .saturating_sub(self.active_registry_unknown_size_count) as u64,
            Some(self.active_registry_known_logical_bytes),
            true,
            true,
        );
        residency.record_resources(
            crate::gpu_residency::ResourceResidency::Active,
            self.active_registry_unknown_size_count as u64,
            None,
            true,
            true,
        );
        residency.record_resources(
            crate::gpu_residency::ResourceResidency::WarmReusable,
            self.warm_registry_mapping_count
                .saturating_sub(self.warm_registry_unknown_size_count) as u64,
            Some(self.warm_registry_known_logical_bytes),
            true,
            true,
        );
        residency.record_resources(
            crate::gpu_residency::ResourceResidency::WarmReusable,
            self.warm_registry_unknown_size_count as u64,
            None,
            true,
            true,
        );
        telemetry.publish_client_import_residency(residency);
    }

    fn diagnostics(&self) -> crate::gpu_residency::RendererDmabufRegistryDiagnostics {
        crate::gpu_residency::RendererDmabufRegistryDiagnostics {
            active_surface_binding_count: self.active_surface_binding_count as u64,
            resident_mapping_count: self.registry.len() as u64,
            resident_known_logical_bytes: self.resident_known_logical_bytes,
            resident_unknown_size_count: self.resident_unknown_size_count as u64,
            active_registry_mapping_count: self.active_registry_mapping_count as u64,
            active_registry_known_logical_bytes: self.active_registry_known_logical_bytes,
            active_registry_unknown_size_count: self.active_registry_unknown_size_count as u64,
            warm_registry_mapping_count: self.warm_registry_mapping_count as u64,
            warm_registry_known_logical_bytes: self.warm_registry_known_logical_bytes,
            warm_registry_unknown_size_count: self.warm_registry_unknown_size_count as u64,
            registry_peak_mapping_count: self.registry_peak_mapping_count as u64,
            registry_peak_known_logical_bytes: self.registry_peak_known_logical_bytes,
            registry_hits_total: self.registry_hits_total,
            registry_misses_total: self.registry_misses_total,
            registry_insertions_total: self.registry_insertions_total,
            registry_retirements_total: self.registry_retirements_total,
            dead_lifetime_evictions_total: self.dead_lifetime_evictions_total,
            imports_total: self.imports_total,
            current_resource_reuses_total: self.current_resource_reuses_total,
            import_failures_total: self.import_failures_total,
        }
    }

    pub(super) fn destroy_all(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
    ) {
        let resources = self.drain_registry_entries();
        for (_, resource) in resources {
            destroy_image_resource(gl, egl, egl_display, resource.image);
        }
    }

    fn drain_registry_entries(
        &mut self,
    ) -> Vec<(DmabufImageKey, DmabufRegistryEntry<EglImageResource>)> {
        let resources = self.registry.drain();
        self.registry_retirements_total = self
            .registry_retirements_total
            .saturating_add(resources.len() as u64);
        self.resident_known_logical_bytes = 0;
        self.resident_unknown_size_count = 0;
        self.active_surface_binding_count = 0;
        self.active_registry_mapping_count = 0;
        self.active_registry_known_logical_bytes = 0;
        self.active_registry_unknown_size_count = 0;
        self.warm_registry_mapping_count = 0;
        self.warm_registry_known_logical_bytes = 0;
        self.warm_registry_unknown_size_count = 0;
        self.failed_surface_generations.clear();
        resources
    }

    fn note_registry_removal(&mut self, key: &DmabufImageKey) {
        if let Some(bytes) = key.logical_payload_bytes() {
            self.resident_known_logical_bytes =
                self.resident_known_logical_bytes.saturating_sub(bytes);
        } else {
            self.resident_unknown_size_count = self.resident_unknown_size_count.saturating_sub(1);
        }
    }

    #[cfg(test)]
    pub(super) fn registry_entry_count(&self) -> usize {
        self.registry.len()
    }

    #[cfg(test)]
    pub(super) fn test_remove_dead_entries(
        &mut self,
    ) -> Vec<(DmabufImageKey, DmabufRegistryEntry<EglImageResource>)> {
        self.remove_dead_entries()
    }

    #[cfg(test)]
    pub(super) fn test_drain_registry_entries(
        &mut self,
    ) -> Vec<(DmabufImageKey, DmabufRegistryEntry<EglImageResource>)> {
        self.drain_registry_entries()
    }

    #[cfg(test)]
    pub(super) fn test_insert_texture(
        &mut self,
        key: DmabufImageKey,
        texture: glow::Texture,
        buffer_lifetime: WeakBufferIdentity,
    ) {
        self.insert_registry_entry(
            key,
            buffer_lifetime,
            EglImageResource {
                texture,
                size: (1, 1),
                generation: 0,
                egl_image: None,
                telemetry: super::image::EglImageResourceTelemetry::Untracked,
            },
            None,
        );
    }
}
