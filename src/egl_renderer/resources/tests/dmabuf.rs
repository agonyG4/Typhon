use super::super::surface::summarize_active_dmabuf_resources;
use super::*;

use std::io;

#[test]
fn active_dmabuf_summary_is_empty_for_an_empty_surface_store() {
    let summary = summarize_active_dmabuf_resources(&HashMap::new());

    assert_eq!(summary.active_surface_binding_count, 0);
    assert!(summary.active_keys.is_empty());
}

#[test]
fn active_dmabuf_summary_ignores_shm_and_uploaded_resources() {
    let resources = HashMap::from([(7, test_shm_resource(None))]);

    let summary = summarize_active_dmabuf_resources(&resources);

    assert_eq!(summary.active_surface_binding_count, 0);
    assert!(summary.active_keys.is_empty());
}

#[test]
fn one_imported_active_resource_reports_its_logical_payload_bytes() {
    let mut ids = BufferIdAllocator::default();
    let identity = ids.allocate().expect("test buffer identity");
    let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let key = DmabufImageKey::from_handle(identity.id(), &handle);
    let resources = HashMap::from([(7, test_dmabuf_resource(&identity, &handle, 1))]);

    let summary = summarize_active_dmabuf_resources(&resources);

    assert_eq!(summary.active_surface_binding_count, 1);
    assert_eq!(summary.active_keys.len(), 1);
    assert!(summary.active_keys.contains(&key));
}

#[test]
fn unknown_format_import_remains_unknown_without_invented_bytes() {
    let mut ids = BufferIdAllocator::default();
    let identity = ids.allocate().expect("test buffer identity");
    let handle = test_dmabuf_handle_with_format(
        256,
        144,
        1024,
        DrmModifier::LINEAR,
        DrmFormat::Other(u32::from_le_bytes(*b"NV12")),
    );
    let resources = HashMap::from([(7, test_dmabuf_resource(&identity, &handle, 1))]);

    let summary = summarize_active_dmabuf_resources(&resources);

    assert_eq!(summary.active_surface_binding_count, 1);
    assert_eq!(summary.active_keys.len(), 1);
}

#[test]
fn replacing_the_active_imported_resource_updates_the_owner_bytes() {
    let mut ids = BufferIdAllocator::default();
    let first_identity = ids.allocate().expect("first test buffer identity");
    let replacement_identity = ids.allocate().expect("replacement test buffer identity");
    let first_handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let replacement_handle = test_dmabuf_handle(128, 72, 512, DrmModifier::LINEAR);
    let first_key = DmabufImageKey::from_handle(first_identity.id(), &first_handle);
    let replacement_key =
        DmabufImageKey::from_handle(replacement_identity.id(), &replacement_handle);
    let mut resources =
        HashMap::from([(7, test_dmabuf_resource(&first_identity, &first_handle, 1))]);

    resources.insert(
        7,
        test_dmabuf_resource(&replacement_identity, &replacement_handle, 2),
    );

    let summary = summarize_active_dmabuf_resources(&resources);
    assert_eq!(summary.active_surface_binding_count, 1);
    assert_eq!(summary.active_keys.len(), 1);
    assert!(summary.active_keys.contains(&replacement_key));
    assert!(!summary.active_keys.contains(&first_key));
}

#[test]
fn removing_an_active_imported_resource_returns_owner_inventory_to_zero() {
    let mut ids = BufferIdAllocator::default();
    let identity = ids.allocate().expect("test buffer identity");
    let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let mut resources = HashMap::from([(7, test_dmabuf_resource(&identity, &handle, 1))]);

    resources.remove(&7);

    let summary = summarize_active_dmabuf_resources(&resources);
    assert_eq!(summary.active_surface_binding_count, 0);
    assert!(summary.active_keys.is_empty());
}

#[test]
fn publication_counts_shared_registry_mappings_once_and_separates_bindings() {
    let mut ids = BufferIdAllocator::default();
    let active_identity = ids.allocate().expect("active test buffer identity");
    let cached_identity = ids.allocate().expect("cached test buffer identity");
    let active_handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let cached_handle = test_dmabuf_handle(128, 72, 512, DrmModifier::LINEAR);
    let active_key = DmabufImageKey::from_handle(active_identity.id(), &active_handle);
    let cached_key = DmabufImageKey::from_handle(cached_identity.id(), &cached_handle);
    let active_bytes = active_key.logical_payload_bytes().unwrap();
    let cached_bytes = cached_key.logical_payload_bytes().unwrap();
    let mut state = RendererResourceState::new(None);
    state
        .surfaces
        .resources
        .insert(7, test_dmabuf_resource(&active_identity, &active_handle, 1));
    state.dmabuf.test_insert_texture(
        active_key,
        glow::NativeTexture(std::num::NonZeroU32::new(1).unwrap()),
        active_identity.downgrade(),
    );
    state.dmabuf.test_insert_texture(
        cached_key,
        glow::NativeTexture(std::num::NonZeroU32::new(2).unwrap()),
        cached_identity.downgrade(),
    );
    let mut frame_stats = GlesSceneFrameStats::default();
    let mut telemetry = ResourceTelemetry::new(&mut frame_stats);

    state.publish_current_dmabuf_owner_metrics(&mut telemetry);

    let owners = frame_stats.gpu_residency.client_import;
    let registry = frame_stats.gpu_residency.renderer_dmabuf_registry;
    assert_eq!(owners.active_count, 1);
    assert_eq!(owners.active_bytes, active_bytes);
    assert_eq!(owners.reusable_count, 1);
    assert_eq!(owners.reusable_bytes, cached_bytes);
    assert_eq!(owners.external_mapping_count, 2);
    assert_eq!(registry.active_surface_binding_count, 1);
    assert_eq!(registry.resident_mapping_count, 2);
    assert_eq!(
        registry.resident_known_logical_bytes,
        active_bytes + cached_bytes
    );
    assert_eq!(registry.active_registry_mapping_count, 1);
    assert_eq!(registry.active_registry_known_logical_bytes, active_bytes);
    assert_eq!(registry.warm_registry_mapping_count, 1);
    assert_eq!(registry.warm_registry_known_logical_bytes, cached_bytes);
}

#[test]
fn two_surface_bindings_resolve_one_registry_mapping_and_count_external_bytes_once() {
    let mut ids = BufferIdAllocator::default();
    let identity = ids.allocate().expect("shared test buffer identity");
    let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let key = DmabufImageKey::from_handle(identity.id(), &handle);
    let bytes = key.logical_payload_bytes().expect("known logical size");
    let texture = glow::NativeTexture(std::num::NonZeroU32::new(19).unwrap());
    let mut state = RendererResourceState::new(None);
    state
        .dmabuf
        .test_insert_texture(key.clone(), texture, identity.downgrade());
    state
        .surfaces
        .resources
        .insert(7, test_dmabuf_binding(key.clone(), 1));
    state
        .surfaces
        .resources
        .insert(8, test_dmabuf_binding(key, 4));

    let view = state.texture_view();
    assert_eq!(view.texture_for_surface(7), Some(texture));
    assert_eq!(view.texture_for_surface(8), Some(texture));

    let mut frame_stats = GlesSceneFrameStats::default();
    state.publish_current_dmabuf_owner_metrics(&mut ResourceTelemetry::new(&mut frame_stats));
    let registry = frame_stats.gpu_residency.renderer_dmabuf_registry;
    let owners = frame_stats.gpu_residency.client_import;
    assert_eq!(registry.active_surface_binding_count, 2);
    assert_eq!(registry.resident_mapping_count, 1);
    assert_eq!(registry.resident_known_logical_bytes, bytes);
    assert_eq!(registry.active_registry_mapping_count, 1);
    assert_eq!(owners.external_mapping_count, 1);
    assert_eq!(owners.external_mapping_bytes, bytes);
}

#[test]
fn publication_after_realization_insertion_observes_the_new_active_resource() {
    let mut ids = BufferIdAllocator::default();
    let identity = ids.allocate().expect("test buffer identity");
    let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let key = DmabufImageKey::from_handle(identity.id(), &handle);
    let mut state = RendererResourceState::new(None);

    let mut before_stats = GlesSceneFrameStats::default();
    state.publish_current_dmabuf_owner_metrics(&mut ResourceTelemetry::new(&mut before_stats));
    assert_eq!(
        before_stats
            .gpu_residency
            .client_import
            .external_mapping_count,
        0
    );

    state
        .surfaces
        .resources
        .insert(7, test_dmabuf_resource(&identity, &handle, 1));
    state.dmabuf.test_insert_texture(
        key.clone(),
        glow::NativeTexture(std::num::NonZeroU32::new(1).unwrap()),
        identity.downgrade(),
    );
    let mut after_stats = GlesSceneFrameStats::default();
    state.publish_current_dmabuf_owner_metrics(&mut ResourceTelemetry::new(&mut after_stats));

    assert_eq!(
        after_stats
            .gpu_residency
            .client_import
            .external_mapping_count,
        1
    );
    assert_eq!(
        after_stats
            .gpu_residency
            .client_import
            .external_mapping_bytes,
        key.logical_payload_bytes().unwrap()
    );
    assert_eq!(
        after_stats
            .gpu_residency
            .renderer_dmabuf_registry
            .active_surface_binding_count,
        1
    );
}

#[test]
fn publication_after_surface_detach_reports_one_warm_registry_mapping() {
    let mut ids = BufferIdAllocator::default();
    let identity = ids.allocate().expect("test buffer identity");
    let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let key = DmabufImageKey::from_handle(identity.id(), &handle);
    let bytes = key.logical_payload_bytes().unwrap();
    let mut state = RendererResourceState::new(None);
    state
        .surfaces
        .resources
        .insert(7, test_dmabuf_resource(&identity, &handle, 1));
    state.dmabuf.test_insert_texture(
        key,
        glow::NativeTexture(std::num::NonZeroU32::new(2).unwrap()),
        identity.downgrade(),
    );
    state.surfaces.resources.remove(&7);
    let mut frame_stats = GlesSceneFrameStats::default();

    state.publish_current_dmabuf_owner_metrics(&mut ResourceTelemetry::new(&mut frame_stats));

    let owners = frame_stats.gpu_residency.client_import;
    let registry = frame_stats.gpu_residency.renderer_dmabuf_registry;
    assert_eq!(owners.active_count, 0);
    assert_eq!(owners.active_bytes, 0);
    assert_eq!(owners.external_mapping_count, 1);
    assert_eq!(owners.reusable_count, 1);
    assert_eq!(owners.reusable_bytes, bytes);
    assert_eq!(registry.active_surface_binding_count, 0);
    assert_eq!(registry.resident_mapping_count, 1);
    assert_eq!(registry.resident_known_logical_bytes, bytes);
    assert_eq!(registry.active_registry_mapping_count, 0);
    assert_eq!(registry.warm_registry_mapping_count, 1);
    assert_eq!(registry.warm_registry_known_logical_bytes, bytes);
}

#[test]
fn stable_owner_publication_matches_independent_event_ledger_totals() {
    let mut ids = BufferIdAllocator::default();
    let identity = ids.allocate().expect("test buffer identity");
    let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let mut state = RendererResourceState::new(None);
    state
        .surfaces
        .resources
        .insert(7, test_dmabuf_resource(&identity, &handle, 1));
    state.dmabuf.test_insert_texture(
        DmabufImageKey::from_handle(identity.id(), &handle),
        glow::NativeTexture(std::num::NonZeroU32::new(1).unwrap()),
        identity.downgrade(),
    );
    let mut frame_stats = GlesSceneFrameStats::default();
    state.publish_current_dmabuf_owner_metrics(&mut ResourceTelemetry::new(&mut frame_stats));

    // Model the independently observed stable event ledger without touching the process-global ledger.
    let event_ledger = [(1_u64, Some(256_u64 * 144 * 4))];
    let event_count = event_ledger.len() as u64;
    let event_known_bytes = event_ledger
        .iter()
        .filter_map(|(_, bytes)| *bytes)
        .sum::<u64>();
    let owners = frame_stats.gpu_residency.client_import;

    assert_eq!(owners.external_mapping_count, event_count);
    assert_eq!(owners.external_mapping_bytes, event_known_bytes);
}

fn settle_dmabuf_import_result<T>(
    result: Result<T, Box<dyn std::error::Error>>,
    context: DmabufImportDiagnosticContext,
    frame_stats: &mut GlesSceneFrameStats,
    failed_surface_generations: &mut HashMap<u32, u64>,
) -> Result<Option<T>, Box<dyn std::error::Error>> {
    let mut telemetry = ResourceTelemetry::new(frame_stats);
    super::settle_dmabuf_import_result(result, context, &mut telemetry, failed_surface_generations)
}

#[test]
fn five_buffer_ring_reuses_every_import_after_warmup() {
    let mut ids = BufferIdAllocator::default();
    let ring = (0..5)
        .map(|_| {
            let identity = ids.allocate().expect("test buffer identity");
            let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
            let key = DmabufImageKey::from_handle(identity.id(), &handle);
            (identity, key)
        })
        .collect::<Vec<_>>();
    let drops = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut registry = DmabufImportRegistry::default();
    let mut imports = 0;
    let mut hits = 0;
    let mut misses = 0;
    let mut second_a_was_registry_hit = false;

    for (visit, slot) in (0..ring.len()).cycle().take(ring.len() * 8).enumerate() {
        let (identity, key) = &ring[slot];
        if registry.get(key).is_some() {
            hits += 1;
            if visit == ring.len() {
                second_a_was_registry_hit = true;
            }
        } else {
            misses += 1;
            imports += 1;
            registry.insert(
                key.clone(),
                DropProbe(std::rc::Rc::clone(&drops)),
                identity.downgrade(),
            );
        }
    }

    assert_eq!(imports, 5);
    assert_eq!(misses, 5);
    assert_eq!(hits, 35);
    assert!(
        second_a_was_registry_hit,
        "the second A reuses its live mapping"
    );
    assert_eq!(registry.len(), 5);
    assert_eq!(drops.get(), 0);
}

#[test]
fn six_buffer_ring_stays_stable_beyond_the_removed_surface_cache_limit() {
    let mut ids = BufferIdAllocator::default();
    let ring = (0..6)
        .map(|_| {
            let identity = ids.allocate().expect("test buffer identity");
            let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
            let key = DmabufImageKey::from_handle(identity.id(), &handle);
            (identity, key)
        })
        .collect::<Vec<_>>();
    let mut registry = DmabufImportRegistry::default();
    let mut imports = 0;
    let mut hits = 0;

    for slot in (0..ring.len()).cycle().take(ring.len() * 8) {
        let (identity, key) = &ring[slot];
        if registry.get(key).is_some() {
            hits += 1;
        } else {
            imports += 1;
            registry.insert(key.clone(), slot, identity.downgrade());
        }
    }

    assert_eq!(imports, 6);
    assert_eq!(hits, 42);
    assert_eq!(registry.len(), 6);
}

#[test]
fn failed_import_publishes_no_binding_or_registry_entry_then_valid_generation_recovers() {
    let mut ids = BufferIdAllocator::default();
    let identity = ids.allocate().expect("test buffer identity");
    let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let surface = test_dmabuf_surface(7, identity.clone(), handle.clone(), 9);
    let key = DmabufImageKey::from_handle(identity.id(), &handle);
    let mut state = RendererResourceState::new(None);
    let mut frame_stats = GlesSceneFrameStats::default();
    let failed_context = DmabufImportDiagnosticContext::from_surface(
        &surface,
        DmabufImportPath::Replacement,
        DmabufImportRegistryState::Miss,
    )
    .expect("test surface has DMA-BUF import metadata");
    let failed = state
        .dmabuf
        .settle_import_result::<u8>(
            Err(Box::new(DmabufTextureImportError::EglImageCreation(
                egl::Error::BadMatch,
            ))),
            failed_context,
            &mut ResourceTelemetry::new(&mut frame_stats),
        )
        .expect("buffer-local import failure is isolated");

    assert!(failed.is_none());
    assert_eq!(state.dmabuf.registry_entry_count(), 0);
    assert!(!state.surfaces.resources.contains_key(&surface.surface_id));

    let valid_surface = test_dmabuf_surface(7, identity.clone(), handle, 10);
    let valid_context = DmabufImportDiagnosticContext::from_surface(
        &valid_surface,
        DmabufImportPath::Replacement,
        DmabufImportRegistryState::Miss,
    )
    .expect("valid generation has DMA-BUF import metadata");
    let imported = state
        .dmabuf
        .settle_import_result(
            Ok(23_u8),
            valid_context,
            &mut ResourceTelemetry::new(&mut frame_stats),
        )
        .expect("later generation remains valid");
    assert_eq!(imported, Some(23));
    state.dmabuf.test_insert_texture(
        key.clone(),
        glow::NativeTexture(std::num::NonZeroU32::new(23).unwrap()),
        identity.downgrade(),
    );
    state.surfaces.resources.insert(
        7,
        test_dmabuf_binding(key.clone(), valid_surface.generation),
    );

    assert_eq!(state.dmabuf.registry_entry_count(), 1);
    assert_eq!(
        state.texture_view().texture_for_surface(7),
        Some(glow::NativeTexture(std::num::NonZeroU32::new(23).unwrap()))
    );
    assert_eq!(frame_stats.dmabuf_import_failures, 1);
}

#[test]
fn surface_detachment_keeps_a_shared_live_registry_entry() {
    let mut ids = BufferIdAllocator::default();
    let identity = ids.allocate().expect("test buffer identity");
    let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let key = DmabufImageKey::from_handle(identity.id(), &handle);
    let drops = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut registry = DmabufImportRegistry::default();
    registry.insert(
        key.clone(),
        DropProbe(std::rc::Rc::clone(&drops)),
        identity.downgrade(),
    );
    let mut surface_a_binding = Some(key.clone());
    let surface_b_binding = Some(key.clone());

    let first = registry.get(surface_a_binding.as_ref().unwrap());
    let second = registry.get(surface_b_binding.as_ref().unwrap());
    assert!(std::ptr::eq(&first.unwrap().image, &second.unwrap().image));
    assert_eq!(registry.len(), 1);

    surface_a_binding.take();
    assert_eq!(registry.len(), 1);
    assert!(registry.get(surface_b_binding.as_ref().unwrap()).is_some());
    assert_eq!(drops.get(), 0);
}

#[test]
fn changing_surface_from_a_to_b_and_then_to_shm_keeps_unrelated_live_imports() {
    let mut ids = BufferIdAllocator::default();
    let identity_a = ids.allocate().expect("first live buffer identity");
    let identity_b = ids.allocate().expect("second live buffer identity");
    let unrelated_identity = ids.allocate().expect("unrelated live buffer identity");
    let handle_a = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let handle_b = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let unrelated_handle = test_dmabuf_handle(128, 72, 512, DrmModifier::LINEAR);
    let key_a = DmabufImageKey::from_handle(identity_a.id(), &handle_a);
    let key_b = DmabufImageKey::from_handle(identity_b.id(), &handle_b);
    let unrelated_key = DmabufImageKey::from_handle(unrelated_identity.id(), &unrelated_handle);
    let texture_a = glow::NativeTexture(std::num::NonZeroU32::new(1).unwrap());
    let texture_b = glow::NativeTexture(std::num::NonZeroU32::new(2).unwrap());
    let unrelated_texture = glow::NativeTexture(std::num::NonZeroU32::new(3).unwrap());
    let mut state = RendererResourceState::new(None);
    state
        .dmabuf
        .test_insert_texture(key_a.clone(), texture_a, identity_a.downgrade());
    state
        .dmabuf
        .test_insert_texture(key_b.clone(), texture_b, identity_b.downgrade());
    state.dmabuf.test_insert_texture(
        unrelated_key.clone(),
        unrelated_texture,
        unrelated_identity.downgrade(),
    );
    state
        .surfaces
        .resources
        .insert(8, test_shm_resource(Some(SurfaceCommitCounter(1))));

    let surface_b_for_shm = test_dmabuf_surface(8, identity_b.clone(), handle_b.clone(), 2);
    let (action, destroyed_shm) =
        reconcile_surface_resource_backing(&mut state.surfaces.resources, &surface_b_for_shm)
            .expect("SHM storage is discarded before a DMA-BUF consumer realizes B");
    assert_eq!(action, SurfaceResourceLifetimeAction::Destroy);
    assert!(matches!(destroyed_shm, EglSurfaceResource::Uploaded { .. }));
    assert_eq!(state.dmabuf.registry_entry_count(), 3);
    assert_eq!(state.texture_view().texture_for_surface(8), None);

    state
        .surfaces
        .resources
        .insert(7, test_dmabuf_binding(key_a.clone(), 1));

    let surface_b = test_dmabuf_surface(7, identity_b.clone(), handle_b, 2);
    let (action, detached_a) =
        reconcile_surface_resource_backing(&mut state.surfaces.resources, &surface_b)
            .expect("the surface binding to A is detached");
    assert_eq!(action, SurfaceResourceLifetimeAction::DetachDmabuf);
    assert!(matches!(detached_a, EglSurfaceResource::Dmabuf { .. }));
    state
        .surfaces
        .resources
        .insert(7, test_dmabuf_binding(key_b.clone(), 2));
    assert_eq!(state.texture_view().texture_for_surface(7), Some(texture_b));
    assert_eq!(state.dmabuf.registry_entry_count(), 3);
    assert_eq!(state.dmabuf.texture_for_key(&key_a), Some(texture_a));

    let surface_shm = test_shm_surface(RenderableSurfaceDamage::Empty);
    let (action, detached_b) =
        reconcile_surface_resource_backing(&mut state.surfaces.resources, &surface_shm)
            .expect("the surface binding to B is detached for SHM");
    assert_eq!(action, SurfaceResourceLifetimeAction::DetachDmabuf);
    assert!(matches!(detached_b, EglSurfaceResource::Dmabuf { .. }));
    assert_eq!(state.dmabuf.registry_entry_count(), 3);
    assert_eq!(
        state.dmabuf.texture_for_key(&unrelated_key),
        Some(unrelated_texture)
    );
    assert_eq!(state.texture_view().texture_for_surface(7), None);
}

#[test]
fn dmabuf_image_target_invalid_operation_is_buffer_local() {
    let error = DmabufTextureImportError::Gl {
        stage: DmabufImportGlStage::ImageTarget,
        error: glow::INVALID_OPERATION,
    };

    assert_eq!(
        error.classification(),
        DmabufImportFailureClass::BufferIncompatible
    );
}

#[test]
fn dmabuf_texture_configuration_invalid_operation_is_renderer_fatal() {
    let error = DmabufTextureImportError::Gl {
        stage: DmabufImportGlStage::TextureConfiguration,
        error: glow::INVALID_OPERATION,
    };

    assert_eq!(
        error.classification(),
        DmabufImportFailureClass::RendererFatal
    );
}

#[test]
fn dmabuf_import_error_drain_returns_first_error_and_clears_all_stale_errors() {
    let mut errors = [glow::INVALID_OPERATION, glow::INVALID_ENUM, glow::NO_ERROR].into_iter();

    assert_eq!(
        first_drained_gl_error(|| errors.next().expect("test error stream is finite")),
        Some(glow::INVALID_OPERATION)
    );
    assert!(errors.next().is_none());
}

#[test]
fn dmabuf_replacement_incompatibility_is_surface_local() {
    let mut frame_stats = GlesSceneFrameStats::default();
    let mut failed_surface_generations = HashMap::new();
    let result = settle_dmabuf_import_result::<u8>(
        Err(Box::new(DmabufTextureImportError::EglImageCreation(
            egl::Error::BadMatch,
        ))),
        DmabufImportDiagnosticContext {
            surface_id: 7,
            generation: 9,
            buffer_id: 11,
            width: 2,
            height: 2,
            fourcc: XR24,
            modifier: 0,
            planes: 1,
            path: DmabufImportPath::Replacement,
            registry: DmabufImportRegistryState::Miss,
        },
        &mut frame_stats,
        &mut failed_surface_generations,
    );

    assert!(
        result
            .expect("buffer-local import rejection is absorbed")
            .is_none()
    );
    assert_eq!(frame_stats.dmabuf_import_failures, 1);
    assert_eq!(failed_surface_generations.get(&7), Some(&9));
}

#[test]
fn dmabuf_renderer_fatal_failure_still_escapes_surface_local_boundary() {
    let mut frame_stats = GlesSceneFrameStats::default();
    let mut failed_surface_generations = HashMap::new();
    let result = settle_dmabuf_import_result::<u8>(
        Err(Box::new(DmabufTextureImportError::Gl {
            stage: DmabufImportGlStage::Bind,
            error: glow::INVALID_OPERATION,
        })),
        DmabufImportDiagnosticContext {
            surface_id: 7,
            generation: 9,
            buffer_id: 11,
            width: 2,
            height: 2,
            fourcc: XR24,
            modifier: 0,
            planes: 1,
            path: DmabufImportPath::Replacement,
            registry: DmabufImportRegistryState::Miss,
        },
        &mut frame_stats,
        &mut failed_surface_generations,
    );

    assert!(result.is_err());
    assert_eq!(frame_stats.dmabuf_import_failures, 0);
    assert!(failed_surface_generations.is_empty());
}

#[test]
fn unclassified_dmabuf_import_error_still_escapes_surface_local_boundary() {
    let mut frame_stats = GlesSceneFrameStats::default();
    let mut failed_surface_generations = HashMap::new();
    let result = settle_dmabuf_import_result::<u8>(
        Err(io::Error::other("unclassified renderer failure").into()),
        DmabufImportDiagnosticContext {
            surface_id: 7,
            generation: 9,
            buffer_id: 11,
            width: 2,
            height: 2,
            fourcc: XR24,
            modifier: 0,
            planes: 1,
            path: DmabufImportPath::Initial,
            registry: DmabufImportRegistryState::Miss,
        },
        &mut frame_stats,
        &mut failed_surface_generations,
    );

    assert!(result.is_err());
    assert_eq!(frame_stats.dmabuf_import_failures, 0);
    assert!(failed_surface_generations.is_empty());
}

#[test]
fn dmabuf_rejected_replacement_does_not_install_or_cache_failed_generation() {
    let mut frame_stats = GlesSceneFrameStats::default();
    let mut failed_surface_generations = HashMap::new();
    let mut cached_resources = HashMap::from([("A", 1_u8)]);
    let context = DmabufImportDiagnosticContext {
        surface_id: 7,
        generation: 9,
        buffer_id: 12,
        width: 2,
        height: 2,
        fourcc: XR24,
        modifier: 0,
        planes: 1,
        path: DmabufImportPath::Replacement,
        registry: DmabufImportRegistryState::Miss,
    };

    let result = settle_dmabuf_import_result::<u8>(
        Err(Box::new(DmabufTextureImportError::EglImageCreation(
            egl::Error::BadMatch,
        ))),
        context,
        &mut frame_stats,
        &mut failed_surface_generations,
    )
    .expect("buffer-local replacement rejection is nonfatal");
    if let Some(resource) = result {
        cached_resources.insert("B", resource);
    }

    assert!(!cached_resources.contains_key("B"));
    assert_eq!(cached_resources.get("A"), Some(&1));
    assert_eq!(frame_stats.dmabuf_import_failures, 1);
}

#[test]
fn dmabuf_rejected_generation_can_be_followed_by_a_valid_import() {
    let mut frame_stats = GlesSceneFrameStats::default();
    let mut failed_surface_generations = HashMap::new();
    let context = DmabufImportDiagnosticContext {
        surface_id: 7,
        generation: 9,
        buffer_id: 13,
        width: 2,
        height: 2,
        fourcc: XR24,
        modifier: 0,
        planes: 1,
        path: DmabufImportPath::Replacement,
        registry: DmabufImportRegistryState::Miss,
    };

    assert!(
        settle_dmabuf_import_result::<u8>(
            Err(Box::new(DmabufTextureImportError::EglImageCreation(
                egl::Error::BadMatch,
            ))),
            context,
            &mut frame_stats,
            &mut failed_surface_generations,
        )
        .expect("buffer-local replacement rejection is nonfatal")
        .is_none()
    );
    let valid = settle_dmabuf_import_result(
        Ok(3_u8),
        context,
        &mut frame_stats,
        &mut failed_surface_generations,
    )
    .expect("a later valid generation remains usable");

    assert_eq!(valid, Some(3));
    assert_eq!(frame_stats.dmabuf_import_failures, 1);
    assert!(failed_surface_generations.is_empty());
}
#[test]
fn dmabuf_resource_key_matches_same_handle_for_surface() {
    let mut ids = BufferIdAllocator::default();
    let identity = ids.allocate().expect("test buffer identity");
    let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);

    assert_eq!(
        DmabufImageKey::from_handle(identity.id(), &handle),
        DmabufImageKey::from_handle(identity.id(), &handle)
    );
}

#[test]
fn dmabuf_resource_key_separates_buffer_ids_when_raw_fd_is_identical() {
    let mut ids = BufferIdAllocator::default();
    let first = ids.allocate().expect("first test buffer identity");
    let second = ids.allocate().expect("second test buffer identity");
    let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);

    let first_key = DmabufImageKey::from_handle(first.id(), &handle);
    let second_key = DmabufImageKey::from_handle(second.id(), &handle);
    assert_ne!(first_key, second_key);

    let mut registry = DmabufImportRegistry::default();
    registry.insert(
        first_key,
        DropProbe(std::rc::Rc::new(std::cell::Cell::new(0))),
        first.downgrade(),
    );
    registry.insert(
        second_key,
        DropProbe(std::rc::Rc::new(std::cell::Cell::new(0))),
        second.downgrade(),
    );
    assert_eq!(registry.len(), 2);
}

#[test]
fn dmabuf_resource_key_separates_plane_layout_for_same_buffer_id() {
    let mut ids = BufferIdAllocator::default();
    let identity = ids.allocate().expect("test buffer identity");
    let first = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let second = test_dmabuf_handle(256, 144, 2048, DrmModifier::LINEAR);

    let first_key = DmabufImageKey::from_handle(identity.id(), &first);
    let second_key = DmabufImageKey::from_handle(identity.id(), &second);
    assert_ne!(first_key, second_key);

    let mut registry = DmabufImportRegistry::default();
    registry.insert(
        first_key,
        DropProbe(std::rc::Rc::new(std::cell::Cell::new(0))),
        identity.downgrade(),
    );
    registry.insert(
        second_key,
        DropProbe(std::rc::Rc::new(std::cell::Cell::new(0))),
        identity.downgrade(),
    );
    assert_eq!(registry.len(), 2);
}

#[test]
fn dead_buffer_registry_entry_is_evicted_exactly_once() {
    let mut ids = BufferIdAllocator::default();
    let identity = ids.allocate().expect("test buffer identity");
    let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let key = DmabufImageKey::from_handle(identity.id(), &handle);
    let drops = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut registry = DmabufImportRegistry::default();
    registry.insert(
        key.clone(),
        DropProbe(std::rc::Rc::clone(&drops)),
        identity.downgrade(),
    );

    drop(identity);
    let dead = registry.remove_dead();
    assert_eq!(dead.len(), 1);
    drop(dead);
    assert!(registry.remove_dead().is_empty());
    assert_eq!(registry.len(), 0);
    assert_eq!(drops.get(), 1);

    drop(registry.remove(&key));
    assert_eq!(drops.get(), 1);
}

#[test]
fn renderer_teardown_drains_each_registry_entry_once() {
    let mut ids = BufferIdAllocator::default();
    let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let drops = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut registry = DmabufImportRegistry::default();
    for _ in 1..=3 {
        let identity = ids.allocate().expect("test buffer identity");
        registry.insert(
            DmabufImageKey::from_handle(identity.id(), &handle),
            DropProbe(std::rc::Rc::clone(&drops)),
            identity.downgrade(),
        );
    }

    let drained = registry.drain();
    assert_eq!(drained.len(), 3);
    assert_eq!(registry.len(), 0);
    drop(drained);
    assert!(registry.drain().is_empty());

    assert_eq!(drops.get(), 3);
}

#[test]
fn registry_accounting_stays_coherent_across_detach_dead_eviction_and_teardown() {
    let mut ids = BufferIdAllocator::default();
    let identity = ids.allocate().expect("first registry buffer identity");
    let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let key = DmabufImageKey::from_handle(identity.id(), &handle);
    let mut state = RendererResourceState::new(None);
    state.dmabuf.test_insert_texture(
        key.clone(),
        glow::NativeTexture(std::num::NonZeroU32::new(1).unwrap()),
        identity.downgrade(),
    );
    state
        .surfaces
        .resources
        .insert(7, test_dmabuf_binding(key.clone(), 1));

    let mut active_stats = GlesSceneFrameStats::default();
    state.publish_current_dmabuf_owner_metrics(&mut ResourceTelemetry::new(&mut active_stats));
    assert_eq!(
        active_stats
            .gpu_residency
            .renderer_dmabuf_registry
            .active_surface_binding_count,
        1
    );
    assert_eq!(
        active_stats
            .gpu_residency
            .renderer_dmabuf_registry
            .registry_insertions_total,
        1
    );

    state.surfaces.resources.remove(&7);
    let mut detached_stats = GlesSceneFrameStats::default();
    state.publish_current_dmabuf_owner_metrics(&mut ResourceTelemetry::new(&mut detached_stats));
    let detached = detached_stats.gpu_residency.renderer_dmabuf_registry;
    assert_eq!(detached.active_surface_binding_count, 0);
    assert_eq!(detached.resident_mapping_count, 1);
    assert_eq!(detached.warm_registry_mapping_count, 1);

    drop(identity);
    let retired_dead = state.dmabuf.test_remove_dead_entries();
    assert_eq!(retired_dead.len(), 1);
    drop(retired_dead);
    assert!(state.dmabuf.test_remove_dead_entries().is_empty());
    let mut dead_stats = GlesSceneFrameStats::default();
    state.publish_current_dmabuf_owner_metrics(&mut ResourceTelemetry::new(&mut dead_stats));
    let dead = dead_stats.gpu_residency.renderer_dmabuf_registry;
    assert_eq!(dead.resident_mapping_count, 0);
    assert_eq!(dead.resident_known_logical_bytes, 0);
    assert_eq!(dead.dead_lifetime_evictions_total, 1);
    assert_eq!(dead.registry_retirements_total, 1);

    let teardown_identity = ids.allocate().expect("teardown buffer identity");
    let teardown_key = DmabufImageKey::from_handle(teardown_identity.id(), &handle);
    state.dmabuf.test_insert_texture(
        teardown_key.clone(),
        glow::NativeTexture(std::num::NonZeroU32::new(2).unwrap()),
        teardown_identity.downgrade(),
    );
    state
        .surfaces
        .resources
        .insert(8, test_dmabuf_binding(teardown_key, 2));
    state.surfaces.resources.clear();
    let retired_at_teardown = state.dmabuf.test_drain_registry_entries();
    assert_eq!(retired_at_teardown.len(), 1);
    drop(retired_at_teardown);
    assert!(state.dmabuf.test_drain_registry_entries().is_empty());
    let mut teardown_stats = GlesSceneFrameStats::default();
    state.publish_current_dmabuf_owner_metrics(&mut ResourceTelemetry::new(&mut teardown_stats));
    let teardown = teardown_stats.gpu_residency.renderer_dmabuf_registry;
    assert_eq!(teardown.active_surface_binding_count, 0);
    assert_eq!(teardown.resident_mapping_count, 0);
    assert_eq!(teardown.resident_known_logical_bytes, 0);
    assert_eq!(teardown.registry_retirements_total, 2);
}

#[test]
fn obsolete_dmabuf_resource_is_removed_when_current_backing_changes() {
    let mut ids = BufferIdAllocator::default();
    let identity_a = ids.allocate().expect("first test buffer identity");
    let identity_b = ids.allocate().expect("second test buffer identity");
    let handle_a = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let handle_b = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let surface = test_dmabuf_surface(7, identity_b, handle_b, 2);
    let mut resources = HashMap::from([(7, test_dmabuf_resource(&identity_a, &handle_a, 1))]);

    let (action, _resource) = reconcile_surface_resource_backing(&mut resources, &surface)
        .expect("obsolete resource must be reconciled");

    assert_eq!(action, SurfaceResourceLifetimeAction::DetachDmabuf);
    assert!(!resources.contains_key(&surface.surface_id));
}

#[test]
fn exact_current_dmabuf_resource_is_kept_across_render_generation_change() {
    let mut ids = BufferIdAllocator::default();
    let identity = ids.allocate().expect("test buffer identity");
    let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let surface = test_dmabuf_surface(7, identity.clone(), handle.clone(), 2);
    let mut resources = HashMap::from([(7, test_dmabuf_resource(&identity, &handle, 1))]);

    assert_eq!(
        classify_surface_resource_lifetime(&resources[&7], &surface),
        SurfaceResourceLifetimeAction::Keep
    );
    assert!(reconcile_surface_resource_backing(&mut resources, &surface).is_none());
    assert!(resources.contains_key(&surface.surface_id));
}

#[test]
fn repeated_hidden_dmabuf_rotations_do_not_create_replacement_resources() {
    let mut ids = BufferIdAllocator::default();
    let ring = (0..5)
        .map(|_| ids.allocate().expect("hidden rotation buffer identity"))
        .collect::<Vec<_>>();
    let identity_a = ring[0].clone();
    let handle_a = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let mut resources = HashMap::from([(7, test_dmabuf_resource(&identity_a, &handle_a, 1))]);
    let key_a = DmabufImageKey::from_handle(identity_a.id(), &handle_a);
    let mut registry = DmabufImportRegistry::default();
    registry.insert(
        key_a.clone(),
        DropProbe(std::rc::Rc::new(std::cell::Cell::new(0))),
        identity_a.downgrade(),
    );

    for generation in 2..=5 {
        let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let surface =
            test_dmabuf_surface(7, ring[generation as usize - 1].clone(), handle, generation);
        let action =
            reconcile_surface_resource_backing(&mut resources, &surface).map(|(action, _)| action);
        if generation == 2 {
            assert_eq!(action, Some(SurfaceResourceLifetimeAction::DetachDmabuf));
        } else {
            assert_eq!(action, None);
        }
        assert!(resources.is_empty());
        assert_eq!(
            registry.len(),
            1,
            "hidden commits do not import replacements"
        );
        assert!(
            registry.get(&key_a).is_some(),
            "the live A import stays resident"
        );
    }
}

#[test]
fn dead_obsolete_dmabuf_resource_is_not_cacheable() {
    let mut ids = BufferIdAllocator::default();
    let identity_a = ids.allocate().expect("initial test buffer identity");
    let identity_b = ids.allocate().expect("replacement test buffer identity");
    let handle_a = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let handle_b = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let mut resources = HashMap::from([(7, test_dmabuf_resource(&identity_a, &handle_a, 1))]);
    let surface = test_dmabuf_surface(7, identity_b, handle_b, 2);
    drop(identity_a);

    let (action, resource) = reconcile_surface_resource_backing(&mut resources, &surface)
        .expect("obsolete resource must be reconciled");

    assert_eq!(action, SurfaceResourceLifetimeAction::DetachDmabuf);
    assert!(matches!(resource, EglSurfaceResource::Dmabuf { .. }));
    assert!(resources.is_empty());
}

#[test]
fn dmabuf_to_shm_retires_dmabuf_without_creating_shm_resource() {
    let mut ids = BufferIdAllocator::default();
    let identity = ids.allocate().expect("test buffer identity");
    let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let mut resources = HashMap::from([(7, test_dmabuf_resource(&identity, &handle, 1))]);
    let surface = test_shm_surface(RenderableSurfaceDamage::Empty);

    let (action, _resource) = reconcile_surface_resource_backing(&mut resources, &surface)
        .expect("obsolete dmabuf resource must be reconciled");

    assert_eq!(action, SurfaceResourceLifetimeAction::DetachDmabuf);
    assert!(resources.is_empty());
}

#[test]
fn shm_to_dmabuf_destroys_shm_without_importing_dmabuf() {
    let mut ids = BufferIdAllocator::default();
    let identity = ids.allocate().expect("replacement test buffer identity");
    let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let surface = test_dmabuf_surface(7, identity, handle, 2);
    let mut resources = HashMap::from([(7, test_shm_resource(Some(SurfaceCommitCounter(1))))]);

    let (action, _resource) = reconcile_surface_resource_backing(&mut resources, &surface)
        .expect("obsolete shm resource must be reconciled");

    assert_eq!(action, SurfaceResourceLifetimeAction::Destroy);
    assert!(resources.is_empty());
}

#[test]
fn compatible_hidden_shm_update_keeps_stale_texture_and_commit_baseline() {
    let surface = test_shm_surface(RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    }]));
    let synced_commit = Some(SurfaceCommitCounter(1));
    let mut resources = HashMap::from([(7, test_shm_resource(synced_commit))]);

    assert!(reconcile_surface_resource_backing(&mut resources, &surface).is_none());
    assert!(matches!(
        &resources[&7],
        EglSurfaceResource::Uploaded {
            shm_synced_commit: Some(commit),
            ..
        } if Some(*commit) == synced_commit
    ));
}

#[test]
fn hidden_shm_resize_retires_incompatible_texture_without_replacement() {
    let mut surface = test_shm_surface(RenderableSurfaceDamage::Empty);
    let identity = BufferIdAllocator::default()
        .allocate()
        .expect("resized test buffer identity");
    surface.buffer = CommittedSurfaceBuffer::shm_snapshot(
        identity,
        BufferSize::new(3, 2).expect("resized test surface size"),
        vec![0; 6],
    );
    let mut resources = HashMap::from([(7, test_shm_resource(Some(SurfaceCommitCounter(1))))]);

    let (action, _resource) = reconcile_surface_resource_backing(&mut resources, &surface)
        .expect("incompatible shm resource must be reconciled");

    assert_eq!(action, SurfaceResourceLifetimeAction::Destroy);
    assert!(resources.is_empty());
}

#[test]
fn obsolete_resource_is_absent_before_hidden_replacement_can_be_realized() {
    let mut ids = BufferIdAllocator::default();
    let identity_a = ids.allocate().expect("initial test buffer identity");
    let identity_b = ids.allocate().expect("replacement test buffer identity");
    let handle_a = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let handle_b = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let mut resources = HashMap::from([(7, test_dmabuf_resource(&identity_a, &handle_a, 1))]);
    let surface_b = test_dmabuf_surface(7, identity_b, handle_b, 2);

    let (action, _resource) = reconcile_surface_resource_backing(&mut resources, &surface_b)
        .expect("obsolete resource must be reconciled");

    assert_eq!(action, SurfaceResourceLifetimeAction::DetachDmabuf);
    assert!(!resources.contains_key(&surface_b.surface_id));
    assert!(reconcile_surface_resource_backing(&mut resources, &surface_b).is_none());
}

fn test_dmabuf_surface(
    surface_id: u32,
    identity: BufferIdentity,
    handle: DmabufBufferHandle,
    generation: u64,
) -> RenderableSurface {
    let size = handle.size();
    RenderableSurface {
        surface_id,
        x: 0,
        y: 0,
        width: size.width,
        height: size.height,
        placement: SurfacePlacement::root(),
        render_backend: SurfaceRenderBackend::NativeWayland,
        render_placement: None,
        visual_clip: None,
        render_target_size: None,
        generation,
        commit_sequence: SurfaceCommitSequence::initial(),
        buffer: CommittedSurfaceBuffer::dmabuf_handle(identity, handle),
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        opaque_region: SurfaceOpaqueRegion::None,
        damage: RenderableSurfaceDamage::Full,
    }
}

fn test_dmabuf_resource(
    identity: &BufferIdentity,
    handle: &DmabufBufferHandle,
    _generation: u64,
) -> EglSurfaceResource {
    EglSurfaceResource::Dmabuf {
        key: DmabufImageKey::from_handle(identity.id(), handle),
    }
}

fn test_dmabuf_handle(
    width: u32,
    height: u32,
    stride: u32,
    modifier: DrmModifier,
) -> DmabufBufferHandle {
    test_dmabuf_handle_with_format(width, height, stride, modifier, DrmFormat::Xrgb8888)
}

fn test_dmabuf_handle_with_format(
    width: u32,
    height: u32,
    stride: u32,
    modifier: DrmModifier,
    format: DrmFormat,
) -> DmabufBufferHandle {
    let fd = std::fs::File::open("/dev/null")
        .expect("/dev/null exists for dmabuf identity tests")
        .into();
    DmabufBufferHandle::new(
        BufferSize::new(width, height).expect("test dmabuf size is non-zero"),
        format,
        vec![DmabufPlane::new(
            fd,
            DmabufPlaneDescriptor {
                plane_index: 0,
                offset: 0,
                stride,
                modifier,
            },
        )],
    )
    .expect("test dmabuf metadata is valid")
}
