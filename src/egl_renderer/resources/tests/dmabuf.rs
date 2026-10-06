use super::*;

use std::io;

fn settle_dmabuf_import_result<T>(
    result: Result<T, Box<dyn std::error::Error>>,
    context: DmabufImportDiagnosticContext,
    frame_stats: &mut GlesSceneFrameStats,
    failed_surface_generations: &mut HashMap<u32, u64>,
) -> Result<Option<T>, Box<dyn std::error::Error>> {
    let mut telemetry = ResourceTelemetry::new(frame_stats);
    super::settle_dmabuf_import_result(result, context, &mut telemetry, failed_surface_generations)
}

#[derive(Debug, Default, PartialEq, Eq)]

struct DmabufRingQualification {
    imports: usize,
    current_resource_reuses: usize,
    cache_hits: usize,
    cache_misses: usize,
    surface_bound_evictions: usize,
    peak_entries: usize,
}

fn qualify_dmabuf_ring(ring_len: usize, cycles: usize) -> DmabufRingQualification {
    let mut current = None;
    let mut cache = std::collections::BTreeSet::new();
    let mut result = DmabufRingQualification::default();
    for buffer in (0..ring_len).cycle().take(ring_len.saturating_mul(cycles)) {
        if current == Some(buffer) {
            result.current_resource_reuses += 1;
            continue;
        }
        if cache.remove(&buffer) {
            result.cache_hits += 1;
        } else {
            result.cache_misses += 1;
            result.imports += 1;
        }
        if let Some(previous) = current {
            if cache.len() >= MAX_CACHED_DMABUF_RESOURCES_PER_SURFACE {
                let oldest = *cache.iter().next().expect("full cache is non-empty");
                cache.remove(&oldest);
                result.surface_bound_evictions += 1;
            }
            cache.insert(previous);
        }
        current = Some(buffer);
        result.peak_entries = result.peak_entries.max(cache.len());
    }
    result
}

#[test]
fn dmabuf_ring_qualification_exposes_capacity_behavior_without_egl() {
    for ring_len in 2..=5 {
        let result = qualify_dmabuf_ring(ring_len, 2);
        assert_eq!(result.imports, ring_len);
        assert_eq!(result.cache_misses, ring_len);
        assert_eq!(result.cache_hits, ring_len);
        assert_eq!(result.current_resource_reuses, 0);
        assert_eq!(result.surface_bound_evictions, 0);
        assert_eq!(result.peak_entries, ring_len - 1);
    }
    assert!(qualify_dmabuf_ring(6, 2).surface_bound_evictions > 0);
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
            cache: DmabufImportCacheState::Miss,
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
            cache: DmabufImportCacheState::Miss,
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
            cache: DmabufImportCacheState::NotChecked,
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
        cache: DmabufImportCacheState::Miss,
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
        cache: DmabufImportCacheState::Miss,
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

    assert_ne!(
        DmabufImageKey::from_handle(first.id(), &handle),
        DmabufImageKey::from_handle(second.id(), &handle)
    );
}

#[test]
fn dmabuf_resource_key_separates_plane_layout_for_same_buffer_id() {
    let mut ids = BufferIdAllocator::default();
    let identity = ids.allocate().expect("test buffer identity");
    let first = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let second = test_dmabuf_handle(256, 144, 2048, DrmModifier::LINEAR);

    assert_ne!(
        DmabufImageKey::from_handle(identity.id(), &first),
        DmabufImageKey::from_handle(identity.id(), &second)
    );
}

#[test]
fn dead_buffer_cache_entry_is_evicted_exactly_once() {
    let mut ids = BufferIdAllocator::default();
    let identity = ids.allocate().expect("test buffer identity");
    let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let key = DmabufImageKey::from_handle(identity.id(), &handle);
    let drops = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut cache = HashMap::from([(
        key.clone(),
        CachedDmabufResource {
            image: DropProbe(std::rc::Rc::clone(&drops)),
            buffer_lifetime: identity.downgrade(),
            surface_id: 7,
        },
    )]);

    drop(identity);
    for dead in dead_cached_dmabuf_keys(&cache) {
        drop(cache.remove(&dead));
    }
    assert!(dead_cached_dmabuf_keys(&cache).is_empty());
    assert_eq!(drops.get(), 1);

    drop(cache.remove(&key));
    assert_eq!(drops.get(), 1);
}

#[test]
fn renderer_cache_recreation_drops_all_previous_generation_entries() {
    let mut ids = BufferIdAllocator::default();
    let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let drops = std::rc::Rc::new(std::cell::Cell::new(0));
    let mut cache = HashMap::new();
    for surface_id in 1..=3 {
        let identity = ids.allocate().expect("test buffer identity");
        cache.insert(
            DmabufImageKey::from_handle(identity.id(), &handle),
            CachedDmabufResource {
                image: DropProbe(std::rc::Rc::clone(&drops)),
                buffer_lifetime: identity.downgrade(),
                surface_id,
            },
        );
    }

    drop(cache);

    assert_eq!(drops.get(), 3);
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

    assert_eq!(action, SurfaceResourceLifetimeAction::DemoteDmabuf);
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
    let identity_a = ids.allocate().expect("initial test buffer identity");
    let handle_a = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
    let mut resources = HashMap::from([(7, test_dmabuf_resource(&identity_a, &handle_a, 1))]);

    for generation in 2..=5 {
        let identity = ids.allocate().expect("rotated test buffer identity");
        let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let surface = test_dmabuf_surface(7, identity, handle, generation);
        let action =
            reconcile_surface_resource_backing(&mut resources, &surface).map(|(action, _)| action);
        if generation == 2 {
            assert_eq!(action, Some(SurfaceResourceLifetimeAction::DemoteDmabuf));
        } else {
            assert_eq!(action, None);
        }
        assert!(resources.is_empty());
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

    assert_eq!(action, SurfaceResourceLifetimeAction::DemoteDmabuf);
    assert!(
        !resource
            .buffer_lifetime
            .expect("dmabuf resource lifetime")
            .is_alive()
    );
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

    assert_eq!(action, SurfaceResourceLifetimeAction::DemoteDmabuf);
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
    assert_eq!(resources[&7].shm_synced_commit, synced_commit);
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

    assert_eq!(action, SurfaceResourceLifetimeAction::DemoteDmabuf);
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
    generation: u64,
) -> EglSurfaceResource {
    let size = handle.size();
    EglSurfaceResource {
        image: EglImageResource {
            texture: glow::NativeTexture(std::num::NonZeroU32::new(1).unwrap()),
            size: (size.width, size.height),
            generation,
            egl_image: Some(fake_egl_image()),
        },
        dmabuf_key: Some(DmabufImageKey::from_handle(identity.id(), handle)),
        buffer_lifetime: Some(identity.downgrade()),
        shm_synced_commit: None,
    }
}

fn test_dmabuf_handle(
    width: u32,
    height: u32,
    stride: u32,
    modifier: DrmModifier,
) -> DmabufBufferHandle {
    let fd = std::fs::File::open("/dev/null")
        .expect("/dev/null exists for dmabuf identity tests")
        .into();
    DmabufBufferHandle::new(
        BufferSize::new(width, height).expect("test dmabuf size is non-zero"),
        DrmFormat::Xrgb8888,
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
