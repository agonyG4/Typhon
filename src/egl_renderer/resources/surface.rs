use std::collections::HashMap;

use khronos_egl as egl;
use oblivion_one::compositor::{RenderableSurface, SurfaceCommitCounter, SurfaceResourceSyncState};
use oblivion_one::render_backend::buffer::{DmabufImageKey, WeakBufferIdentity};

use super::super::{EglInstance, RendererResult, SurfaceConsumerPlan, native_egl_debug_enabled};
use super::dmabuf_import::{
    DmabufImportCacheState, DmabufImportDiagnosticContext, DmabufImportPath,
    log_dmabuf_import_context,
};
use super::image::{
    EglImageResource, create_uploaded_resource, destroy_image_resource,
    write_surface_pixels_to_resource,
};
use super::upload::UploadScratch;
use super::{RendererResourceState, ResourceTelemetry};

pub(in crate::egl_renderer) struct SurfaceResourceInputs<'a> {
    pub(in crate::egl_renderer) canonical: &'a [RenderableSurface],
    pub(in crate::egl_renderer) lifecycle: &'a [RenderableSurface],
    pub(in crate::egl_renderer) client_cursor: Option<&'a RenderableSurface>,
}

pub(super) struct SurfaceResourceStore {
    pub(super) resources: HashMap<u32, EglSurfaceResource>,
    active_surface_ids: Vec<u32>,
}

impl Default for SurfaceResourceStore {
    fn default() -> Self {
        Self {
            resources: HashMap::new(),
            active_surface_ids: Vec::new(),
        }
    }
}

impl SurfaceResourceStore {
    pub(super) fn destroy_all(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
    ) {
        for (_, resource) in self.resources.drain() {
            destroy_surface_resource(gl, egl, egl_display, resource);
        }
    }
}

pub(super) struct EglSurfaceResource {
    pub(super) image: EglImageResource,
    pub(super) dmabuf_key: Option<DmabufImageKey>,
    pub(super) buffer_lifetime: Option<WeakBufferIdentity>,
    pub(super) shm_synced_commit: Option<SurfaceCommitCounter>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SurfaceResourceLifetimeAction {
    Keep,
    DemoteDmabuf,
    Destroy,
}

impl EglSurfaceResource {
    pub(super) fn advance_shm_sync_baseline(&mut self, synced_commit: SurfaceCommitCounter) {
        self.shm_synced_commit = Some(synced_commit);
    }

    pub(super) fn update_for(
        &self,
        surface: &RenderableSurface,
        sync_state: SurfaceResourceSyncState,
    ) -> EglSurfaceResourceUpdate {
        let buffer_size = surface.buffer_size();
        if self.image.size != (buffer_size.width, buffer_size.height) {
            return EglSurfaceResourceUpdate::Recreate;
        }
        if surface.cpu_pixels().is_some() {
            if self.image.egl_image.is_some() {
                return EglSurfaceResourceUpdate::Recreate;
            }
            if !sync_state.authoritative {
                return EglSurfaceResourceUpdate::FullShmResync;
            }
            if self.shm_synced_commit == Some(sync_state.current_commit) {
                return EglSurfaceResourceUpdate::Reuse;
            }
            if surface.damage.is_history_lost() {
                return EglSurfaceResourceUpdate::FullShmResync;
            }
            if sync_state.complete_since.is_some_and(|complete_since| {
                self.shm_synced_commit
                    .is_some_and(|synced| synced >= complete_since)
            }) {
                return if surface.damage.is_empty() {
                    EglSurfaceResourceUpdate::ReuseShm
                } else {
                    EglSurfaceResourceUpdate::UploadDamage
                };
            }
            return EglSurfaceResourceUpdate::FullShmResync;
        }
        if self.image.generation == surface.generation {
            return EglSurfaceResourceUpdate::Reuse;
        }
        if surface
            .dmabuf_handle()
            .map(|handle| DmabufImageKey::from_handle(surface.buffer_id(), handle))
            .is_some_and(|key| self.dmabuf_key.as_ref() == Some(&key))
        {
            return EglSurfaceResourceUpdate::ReuseDmabuf;
        }
        if surface.dmabuf_handle().is_some() {
            return EglSurfaceResourceUpdate::Recreate;
        }
        EglSurfaceResourceUpdate::UnsupportedBuffer
    }

    fn write_shm_damage(
        &mut self,
        gl: &glow::Context,
        surface: &RenderableSurface,
        force_full_upload: bool,
        synced_commit: SurfaceCommitCounter,
        upload: &mut UploadScratch,
    ) -> usize {
        let upload_bytes =
            write_surface_pixels_to_resource(gl, &self.image, surface, force_full_upload, upload);
        self.image.generation = surface.generation;
        self.shm_synced_commit = Some(synced_commit);
        upload_bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EglSurfaceResourceUpdate {
    Reuse,
    ReuseShm,
    ReuseDmabuf,
    UploadDamage,
    FullShmResync,
    Recreate,
    UnsupportedBuffer,
}

pub(super) fn classify_surface_resource_lifetime(
    resource: &EglSurfaceResource,
    surface: &RenderableSurface,
) -> SurfaceResourceLifetimeAction {
    if let Some(installed_key) = resource.dmabuf_key.as_ref() {
        let current_key = surface
            .dmabuf_handle()
            .map(|handle| DmabufImageKey::from_handle(surface.buffer_id(), handle));
        return if current_key.as_ref() == Some(installed_key) {
            SurfaceResourceLifetimeAction::Keep
        } else {
            SurfaceResourceLifetimeAction::DemoteDmabuf
        };
    }

    let size = surface.buffer_size();
    if surface.cpu_pixels().is_some() && resource.image.size == (size.width, size.height) {
        SurfaceResourceLifetimeAction::Keep
    } else {
        SurfaceResourceLifetimeAction::Destroy
    }
}

pub(super) fn reconcile_surface_resource_backing(
    surface_resources: &mut HashMap<u32, EglSurfaceResource>,
    surface: &RenderableSurface,
) -> Option<(SurfaceResourceLifetimeAction, EglSurfaceResource)> {
    let action = surface_resources
        .get(&surface.surface_id)
        .map(|resource| classify_surface_resource_lifetime(resource, surface))?;
    if action == SurfaceResourceLifetimeAction::Keep {
        return None;
    }
    surface_resources
        .remove(&surface.surface_id)
        .map(|resource| (action, resource))
}

fn destroy_surface_resource(
    gl: &glow::Context,
    egl: &EglInstance,
    egl_display: egl::Display,
    resource: EglSurfaceResource,
) {
    destroy_image_resource(gl, egl, egl_display, resource.image);
}

impl RendererResourceState {
    pub(in crate::egl_renderer) fn reconcile_surface_resource_lifetimes(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
        surfaces: &[RenderableSurface],
        lifecycle_surfaces: &[RenderableSurface],
        client_cursor: Option<&RenderableSurface>,
        telemetry: &mut ResourceTelemetry<'_>,
    ) -> RendererResult<()> {
        self.dmabuf.evict_dead(gl, egl, egl_display, telemetry);
        self.surfaces.active_surface_ids.clear();
        self.surfaces
            .active_surface_ids
            .extend(surfaces.iter().map(|surface| surface.surface_id));
        self.surfaces
            .active_surface_ids
            .extend(lifecycle_surfaces.iter().map(|surface| surface.surface_id));
        self.surfaces
            .active_surface_ids
            .extend(client_cursor.map(|surface| surface.surface_id));
        self.surfaces.active_surface_ids.sort_unstable();
        self.surfaces.active_surface_ids.dedup();

        for surface in surfaces
            .iter()
            .chain(lifecycle_surfaces)
            .chain(client_cursor)
        {
            let Some((action, resource)) =
                reconcile_surface_resource_backing(&mut self.surfaces.resources, surface)
            else {
                continue;
            };
            match action {
                SurfaceResourceLifetimeAction::DemoteDmabuf => {
                    self.cache_or_destroy_dmabuf_resource(
                        gl,
                        egl,
                        egl_display,
                        surface.surface_id,
                        resource,
                        telemetry,
                    );
                }
                SurfaceResourceLifetimeAction::Destroy => {
                    destroy_surface_resource(gl, egl, egl_display, resource);
                }
                SurfaceResourceLifetimeAction::Keep => {
                    unreachable!("current surface resource reconciliation never removes Keep")
                }
            }
        }

        let stale_ids = self
            .surfaces
            .resources
            .keys()
            .copied()
            .filter(|id| self.surfaces.active_surface_ids.binary_search(id).is_err())
            .collect::<Vec<_>>();
        for surface_id in stale_ids {
            if let Some(resource) = self.surfaces.resources.remove(&surface_id) {
                destroy_surface_resource(gl, egl, egl_display, resource);
            }
            self.dmabuf
                .destroy_cached_for_surface(gl, egl, egl_display, surface_id, telemetry);
            self.dmabuf.failed_surface_generations.remove(&surface_id);
        }

        self.dmabuf.publish_metrics(telemetry);
        Ok(())
    }

    pub(in crate::egl_renderer) fn realize_surface_resources_for_consumers(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
        inputs: SurfaceResourceInputs<'_>,
        consumers: &SurfaceConsumerPlan,
        sync_states: &[SurfaceResourceSyncState],
        telemetry: &mut ResourceTelemetry<'_>,
    ) -> RendererResult<()> {
        for surface in inputs
            .canonical
            .iter()
            .chain(inputs.lifecycle)
            .chain(inputs.client_cursor)
        {
            if consumers
                .surface_ids()
                .binary_search(&surface.surface_id)
                .is_err()
            {
                continue;
            }
            let sync_state = sync_states
                .iter()
                .find(|state| state.surface_id == surface.surface_id)
                .copied()
                .unwrap_or(SurfaceResourceSyncState {
                    surface_id: surface.surface_id,
                    complete_since: None,
                    current_commit: SurfaceCommitCounter::default(),
                    authoritative: false,
                });
            self.realize_surface_resource(gl, egl, egl_display, surface, sync_state, telemetry)?;
        }
        self.dmabuf.publish_metrics(telemetry);
        Ok(())
    }

    fn realize_surface_resource(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
        surface: &RenderableSurface,
        sync_state: SurfaceResourceSyncState,
        telemetry: &mut ResourceTelemetry<'_>,
    ) -> RendererResult<()> {
        let update = self
            .surfaces
            .resources
            .get(&surface.surface_id)
            .map_or(EglSurfaceResourceUpdate::Recreate, |resource| {
                resource.update_for(surface, sync_state)
            });
        match update {
            EglSurfaceResourceUpdate::Reuse => return Ok(()),
            EglSurfaceResourceUpdate::ReuseShm => {
                if let Some(resource) = self.surfaces.resources.get_mut(&surface.surface_id) {
                    resource.advance_shm_sync_baseline(sync_state.current_commit);
                }
                return Ok(());
            }
            EglSurfaceResourceUpdate::ReuseDmabuf => {
                if let Some(resource) = self.surfaces.resources.get_mut(&surface.surface_id) {
                    resource.image.generation = surface.generation;
                }
                telemetry.dmabuf_current_resource_reuse();
                telemetry.dmabuf_reuse();
                return Ok(());
            }
            EglSurfaceResourceUpdate::UploadDamage | EglSurfaceResourceUpdate::FullShmResync => {
                if let Some(resource) = self.surfaces.resources.get_mut(&surface.surface_id) {
                    let force_full = update == EglSurfaceResourceUpdate::FullShmResync;
                    telemetry.shm_upload_bytes(resource.write_shm_damage(
                        gl,
                        surface,
                        force_full,
                        sync_state.current_commit,
                        &mut self.upload,
                    ));
                    if force_full {
                        telemetry.shm_full_resync();
                    }
                }
                return Ok(());
            }
            EglSurfaceResourceUpdate::Recreate if surface.dmabuf_handle().is_some() => {
                self.switch_dmabuf_surface_resource(gl, egl, egl_display, surface, telemetry)?;
                return Ok(());
            }
            EglSurfaceResourceUpdate::Recreate => {}
            EglSurfaceResourceUpdate::UnsupportedBuffer => {
                if let Some(resource) = self.surfaces.resources.remove(&surface.surface_id) {
                    destroy_surface_resource(gl, egl, egl_display, resource);
                }
                self.dmabuf.destroy_cached_for_surface(
                    gl,
                    egl,
                    egl_display,
                    surface.surface_id,
                    telemetry,
                );
                return Ok(());
            }
        }

        if let Some(old) = self.surfaces.resources.remove(&surface.surface_id) {
            destroy_surface_resource(gl, egl, egl_display, old);
        }
        if surface.dmabuf_handle().is_none() {
            self.dmabuf.destroy_cached_for_surface(
                gl,
                egl,
                egl_display,
                surface.surface_id,
                telemetry,
            );
        }

        let result = create_surface_resource(
            gl,
            egl,
            egl_display,
            self.dmabuf.importer(),
            surface,
            (surface.cpu_pixels().is_some() && sync_state.authoritative)
                .then_some(sync_state.current_commit),
            &mut self.upload,
        );
        if let Some(context) = DmabufImportDiagnosticContext::from_surface(
            surface,
            DmabufImportPath::Initial,
            DmabufImportCacheState::NotChecked,
        ) {
            if let Some(resource) = self
                .dmabuf
                .settle_import_result(result, context, telemetry)?
            {
                telemetry.dmabuf_imported();
                self.surfaces.resources.insert(surface.surface_id, resource);
            }
        } else {
            match result {
                Ok(resource) => {
                    telemetry.shm_upload_bytes(surface_upload_byte_len(surface));
                    self.dmabuf
                        .failed_surface_generations
                        .remove(&surface.surface_id);
                    self.surfaces.resources.insert(surface.surface_id, resource);
                }
                Err(error) => {
                    eprintln!(
                        "oblivion-one compositor: failed to realize surface {} on EGL/GLES: {error}",
                        surface.surface_id
                    );
                }
            }
        }
        Ok(())
    }

    fn switch_dmabuf_surface_resource(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
        surface: &RenderableSurface,
        telemetry: &mut ResourceTelemetry<'_>,
    ) -> RendererResult<()> {
        let Some(handle) = surface.dmabuf_handle() else {
            return Ok(());
        };
        let key = DmabufImageKey::from_handle(surface.buffer_id(), handle);

        if let Some(mut cached) = self.dmabuf.take_cached(&key) {
            if native_egl_debug_enabled() {
                eprintln!(
                    "oblivion-one compositor: dmabuf cache=hit surface={} key={key:?} texture={:?} egl_image={:?}",
                    surface.surface_id,
                    cached.image.texture,
                    cached.image.egl_image.map(|image| image.as_ptr()),
                );
                if let Some(context) = DmabufImportDiagnosticContext::from_surface(
                    surface,
                    DmabufImportPath::Replacement,
                    DmabufImportCacheState::Hit,
                ) {
                    log_dmabuf_import_context(
                        context,
                        "resource_selected",
                        "cache",
                        cached.image.egl_image.is_some(),
                        None,
                        "success",
                    );
                }
            }
            cached.image.generation = surface.generation;
            self.dmabuf.note_cache_hit(telemetry);
            telemetry.dmabuf_reuse();
            if let Some(old) = self.surfaces.resources.insert(
                surface.surface_id,
                EglSurfaceResource {
                    image: cached.image,
                    dmabuf_key: Some(key),
                    buffer_lifetime: Some(surface.buffer_identity().downgrade()),
                    shm_synced_commit: None,
                },
            ) {
                self.cache_or_destroy_dmabuf_resource(
                    gl,
                    egl,
                    egl_display,
                    surface.surface_id,
                    old,
                    telemetry,
                );
            }
            return Ok(());
        }

        if native_egl_debug_enabled() {
            eprintln!(
                "oblivion-one compositor: dmabuf cache=miss surface={} key={key:?} layout={:?}",
                surface.surface_id,
                surface.dmabuf_handle(),
            );
        }
        self.dmabuf.note_cache_miss(telemetry);

        let Some(old) = self.surfaces.resources.remove(&surface.surface_id) else {
            let result = create_surface_resource(
                gl,
                egl,
                egl_display,
                self.dmabuf.importer(),
                surface,
                None,
                &mut self.upload,
            );
            let context = DmabufImportDiagnosticContext::from_surface(
                surface,
                DmabufImportPath::Initial,
                DmabufImportCacheState::Miss,
            )
            .expect("switching a DMA-BUF resource requires a DMA-BUF surface");
            if let Some(resource) = self
                .dmabuf
                .settle_import_result(result, context, telemetry)?
            {
                telemetry.dmabuf_imported();
                self.surfaces.resources.insert(surface.surface_id, resource);
            }
            return Ok(());
        };
        self.cache_or_destroy_dmabuf_resource(
            gl,
            egl,
            egl_display,
            surface.surface_id,
            old,
            telemetry,
        );

        let result = create_surface_resource(
            gl,
            egl,
            egl_display,
            self.dmabuf.importer(),
            surface,
            None,
            &mut self.upload,
        );
        let context = DmabufImportDiagnosticContext::from_surface(
            surface,
            DmabufImportPath::Replacement,
            DmabufImportCacheState::Miss,
        )
        .expect("switching a DMA-BUF resource requires a DMA-BUF surface");
        if let Some(resource) = self
            .dmabuf
            .settle_import_result(result, context, telemetry)?
        {
            telemetry.dmabuf_imported();
            self.surfaces.resources.insert(surface.surface_id, resource);
        }
        Ok(())
    }

    fn cache_or_destroy_dmabuf_resource(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
        surface_id: u32,
        resource: EglSurfaceResource,
        telemetry: &mut ResourceTelemetry<'_>,
    ) {
        self.dmabuf.cache_or_destroy_resource(
            gl,
            egl,
            egl_display,
            surface_id,
            resource.dmabuf_key,
            resource.buffer_lifetime,
            resource.image,
            telemetry,
        );
    }
}

fn create_surface_resource(
    gl: &glow::Context,
    egl: &EglInstance,
    egl_display: egl::Display,
    importer: &super::dmabuf_import::DmabufImporter,
    surface: &RenderableSurface,
    shm_synced_commit: Option<SurfaceCommitCounter>,
    upload: &mut UploadScratch,
) -> RendererResult<EglSurfaceResource> {
    let image = if surface.cpu_pixels().is_some() {
        let buffer_size = surface.buffer_size();
        let mut resource = create_uploaded_resource(gl, buffer_size.width, buffer_size.height)?;
        write_surface_pixels_to_resource(gl, &resource, surface, true, upload);
        resource.generation = surface.generation;
        resource
    } else if let Some(handle) = surface.dmabuf_handle() {
        importer.create_resource(gl, egl, egl_display, handle, surface.generation)?
    } else {
        return Err(std::io::Error::other("surface has no importable buffer").into());
    };

    if native_egl_debug_enabled() && surface.dmabuf_handle().is_some() {
        eprintln!(
            "oblivion-one compositor: dmabuf cache=create surface={} buffer_id={} texture={:?} egl_image={:?}",
            surface.surface_id,
            surface.buffer_id().get(),
            image.texture,
            image.egl_image.map(|egl_image| egl_image.as_ptr()),
        );
    }

    Ok(EglSurfaceResource {
        image,
        dmabuf_key: surface
            .dmabuf_handle()
            .map(|handle| DmabufImageKey::from_handle(surface.buffer_id(), handle)),
        buffer_lifetime: surface
            .dmabuf_handle()
            .map(|_| surface.buffer_identity().downgrade()),
        shm_synced_commit,
    })
}

fn surface_upload_byte_len(surface: &RenderableSurface) -> usize {
    let size = surface.buffer_size();
    (size.width as usize)
        .saturating_mul(size.height as usize)
        .saturating_mul(4)
}
