use std::collections::HashMap;

use khronos_egl as egl;
use oblivion_one::compositor::{RenderableSurface, SurfaceCommitCounter, SurfaceResourceSyncState};
use oblivion_one::render_backend::buffer::DmabufImageKey;

use super::super::{EglInstance, RendererResult, SurfaceConsumerPlan, native_egl_debug_enabled};
use super::dmabuf_import::{
    DmabufImportDiagnosticContext, DmabufImportPath, DmabufImportRegistryState,
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

pub(super) enum EglSurfaceResource {
    Uploaded {
        image: EglImageResource,
        shm_synced_commit: Option<SurfaceCommitCounter>,
    },
    Dmabuf {
        key: DmabufImageKey,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ActiveDmabufResourceSummary {
    pub(super) active_surface_binding_count: usize,
    pub(super) active_keys: std::collections::HashSet<DmabufImageKey>,
}

pub(super) fn summarize_active_dmabuf_resources(
    resources: &HashMap<u32, EglSurfaceResource>,
) -> ActiveDmabufResourceSummary {
    let mut summary = ActiveDmabufResourceSummary::default();
    for resource in resources.values() {
        let EglSurfaceResource::Dmabuf { key } = resource else {
            continue;
        };
        summary.active_surface_binding_count =
            summary.active_surface_binding_count.saturating_add(1);
        summary.active_keys.insert(key.clone());
    }
    summary
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SurfaceResourceLifetimeAction {
    Keep,
    DetachDmabuf,
    Destroy,
}

impl EglSurfaceResource {
    pub(super) fn advance_shm_sync_baseline(&mut self, synced_commit: SurfaceCommitCounter) {
        if let Self::Uploaded {
            shm_synced_commit, ..
        } = self
        {
            *shm_synced_commit = Some(synced_commit);
        }
    }

    pub(super) fn update_for(
        &self,
        surface: &RenderableSurface,
        sync_state: SurfaceResourceSyncState,
    ) -> EglSurfaceResourceUpdate {
        match self {
            Self::Uploaded {
                image,
                shm_synced_commit,
            } => {
                let buffer_size = surface.buffer_size();
                if image.size != (buffer_size.width, buffer_size.height) {
                    return EglSurfaceResourceUpdate::Recreate;
                }
                if surface.cpu_pixels().is_none() {
                    return if surface.dmabuf_handle().is_some() {
                        EglSurfaceResourceUpdate::Recreate
                    } else {
                        EglSurfaceResourceUpdate::UnsupportedBuffer
                    };
                }
                if image.egl_image.is_some() {
                    return EglSurfaceResourceUpdate::Recreate;
                }
                if !sync_state.authoritative {
                    return EglSurfaceResourceUpdate::FullShmResync;
                }
                if *shm_synced_commit == Some(sync_state.current_commit) {
                    return EglSurfaceResourceUpdate::Reuse;
                }
                if surface.damage.is_history_lost() {
                    return EglSurfaceResourceUpdate::FullShmResync;
                }
                if sync_state.complete_since.is_some_and(|complete_since| {
                    shm_synced_commit.is_some_and(|synced| synced >= complete_since)
                }) {
                    return if surface.damage.is_empty() {
                        EglSurfaceResourceUpdate::ReuseShm
                    } else {
                        EglSurfaceResourceUpdate::UploadDamage
                    };
                }
                EglSurfaceResourceUpdate::FullShmResync
            }
            Self::Dmabuf { key } => {
                if surface.cpu_pixels().is_some() {
                    return EglSurfaceResourceUpdate::Recreate;
                }
                if surface
                    .dmabuf_handle()
                    .map(|handle| DmabufImageKey::from_handle(surface.buffer_id(), handle))
                    .is_some_and(|current_key| &current_key == key)
                {
                    EglSurfaceResourceUpdate::ReuseDmabuf
                } else if surface.dmabuf_handle().is_some() {
                    EglSurfaceResourceUpdate::Recreate
                } else {
                    EglSurfaceResourceUpdate::UnsupportedBuffer
                }
            }
        }
    }

    fn write_shm_damage(
        &mut self,
        gl: &glow::Context,
        surface: &RenderableSurface,
        force_full_upload: bool,
        synced_commit: SurfaceCommitCounter,
        upload: &mut UploadScratch,
    ) -> usize {
        let Self::Uploaded {
            image,
            shm_synced_commit,
        } = self
        else {
            unreachable!("only uploaded surface resources can receive SHM damage")
        };
        let upload_bytes =
            write_surface_pixels_to_resource(gl, image, surface, force_full_upload, upload);
        image.generation = surface.generation;
        *shm_synced_commit = Some(synced_commit);
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
    match resource {
        EglSurfaceResource::Dmabuf { key } => {
            let current_key = surface
                .dmabuf_handle()
                .map(|handle| DmabufImageKey::from_handle(surface.buffer_id(), handle));
            if current_key.as_ref() == Some(key) {
                SurfaceResourceLifetimeAction::Keep
            } else {
                SurfaceResourceLifetimeAction::DetachDmabuf
            }
        }
        EglSurfaceResource::Uploaded { image, .. } => {
            let size = surface.buffer_size();
            if surface.cpu_pixels().is_some() && image.size == (size.width, size.height) {
                SurfaceResourceLifetimeAction::Keep
            } else {
                SurfaceResourceLifetimeAction::Destroy
            }
        }
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
    if let EglSurfaceResource::Uploaded { image, .. } = resource {
        destroy_image_resource(gl, egl, egl_display, image);
    }
}

impl RendererResourceState {
    pub(super) fn publish_current_dmabuf_owner_metrics(
        &mut self,
        telemetry: &mut ResourceTelemetry<'_>,
    ) {
        let summary = summarize_active_dmabuf_resources(&self.surfaces.resources);
        self.dmabuf.publish_metrics(
            summary.active_surface_binding_count,
            &summary.active_keys,
            telemetry,
        );
    }

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
                SurfaceResourceLifetimeAction::DetachDmabuf => {}
                SurfaceResourceLifetimeAction::Destroy => {
                    destroy_surface_resource(gl, egl, egl_display, resource);
                }
                SurfaceResourceLifetimeAction::Keep => {
                    unreachable!("current surface resource reconciliation never removes Keep")
                }
            }
        }

        let mut stale_ids = Vec::new();
        for &surface_id in self.surfaces.resources.keys() {
            if self
                .surfaces
                .active_surface_ids
                .binary_search(&surface_id)
                .is_err()
            {
                stale_ids.push(surface_id);
            }
        }
        for surface_id in stale_ids {
            if let Some(resource) = self.surfaces.resources.remove(&surface_id) {
                destroy_surface_resource(gl, egl, egl_display, resource);
            }
            self.dmabuf.failed_surface_generations.remove(&surface_id);
        }

        self.publish_current_dmabuf_owner_metrics(telemetry);
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
        self.publish_current_dmabuf_owner_metrics(telemetry);
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
                let key = surface
                    .dmabuf_handle()
                    .map(|handle| DmabufImageKey::from_handle(surface.buffer_id(), handle));
                if key
                    .as_ref()
                    .is_some_and(|key| self.dmabuf.contains_live_key(key))
                {
                    self.dmabuf.note_current_resource_reuse(telemetry);
                    return Ok(());
                }
                self.switch_dmabuf_surface_resource(gl, egl, egl_display, surface, telemetry)?;
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
                return Ok(());
            }
        }

        if let Some(old) = self.surfaces.resources.remove(&surface.surface_id) {
            destroy_surface_resource(gl, egl, egl_display, old);
        }
        let result = create_surface_resource(
            gl,
            surface,
            (surface.cpu_pixels().is_some() && sync_state.authoritative)
                .then_some(sync_state.current_commit),
            &mut self.upload,
        );
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
        self.dmabuf.evict_dead(gl, egl, egl_display, telemetry);
        let previous = self.surfaces.resources.remove(&surface.surface_id);
        let path = if previous.is_some() {
            DmabufImportPath::Replacement
        } else {
            DmabufImportPath::Initial
        };

        if self.dmabuf.lookup_texture(&key, telemetry).is_some() {
            if native_egl_debug_enabled()
                && let Some(context) = DmabufImportDiagnosticContext::from_surface(
                    surface,
                    path,
                    DmabufImportRegistryState::Hit,
                )
            {
                log_dmabuf_import_context(
                    context,
                    "resource_selected",
                    "registry",
                    true,
                    None,
                    "success",
                );
            }
            let old = self
                .surfaces
                .resources
                .insert(surface.surface_id, EglSurfaceResource::Dmabuf { key });
            if let Some(old) = old.or(previous) {
                destroy_surface_resource(gl, egl, egl_display, old);
            }
            return Ok(());
        }

        if native_egl_debug_enabled() {
            eprintln!(
                "oblivion-one compositor: dmabuf registry=miss surface={} key={key:?} layout={:?}",
                surface.surface_id,
                surface.dmabuf_handle(),
            );
        }
        if let Some(old) = previous {
            destroy_surface_resource(gl, egl, egl_display, old);
        }

        let result = self
            .dmabuf
            .importer()
            .create_resource(gl, egl, egl_display, handle);
        let context = DmabufImportDiagnosticContext::from_surface(
            surface,
            path,
            DmabufImportRegistryState::Miss,
        )
        .expect("switching a DMA-BUF resource requires a DMA-BUF surface");
        if let Some(image) = self
            .dmabuf
            .settle_import_result(result, context, telemetry)?
        {
            self.dmabuf.note_import_success(telemetry);
            self.dmabuf.insert_import(
                key.clone(),
                surface.buffer_identity().downgrade(),
                image,
                telemetry,
            );
            self.surfaces
                .resources
                .insert(surface.surface_id, EglSurfaceResource::Dmabuf { key });
        }
        Ok(())
    }
}

fn create_surface_resource(
    gl: &glow::Context,
    surface: &RenderableSurface,
    shm_synced_commit: Option<SurfaceCommitCounter>,
    upload: &mut UploadScratch,
) -> RendererResult<EglSurfaceResource> {
    if surface.cpu_pixels().is_none() {
        return Err(std::io::Error::other("surface has no importable buffer").into());
    }

    let buffer_size = surface.buffer_size();
    let mut image = create_uploaded_resource(
        gl,
        buffer_size.width,
        buffer_size.height,
        crate::gpu_residency::GpuResourceClass::ShmUploadTextureBacking,
    )?;
    write_surface_pixels_to_resource(gl, &image, surface, true, upload);
    image.generation = surface.generation;

    Ok(EglSurfaceResource::Uploaded {
        image,
        shm_synced_commit,
    })
}

fn surface_upload_byte_len(surface: &RenderableSurface) -> usize {
    let size = surface.buffer_size();
    (size.width as usize)
        .saturating_mul(size.height as usize)
        .saturating_mul(4)
}
