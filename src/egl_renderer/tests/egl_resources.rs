use super::*;

pub(super) fn fake_egl_image() -> egl::Image {
    // SAFETY: the fake handle is only passed to the test cleanup probe;
    // it is never sent to EGL.
    unsafe { egl::Image::from_ptr(std::ptr::NonNull::<c_void>::dangling().as_ptr()) }
}

#[test]
fn failed_image_creation_has_no_cleanup_owner() {
    let image_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
    let image_result: Result<egl::Image, ()> = Err(());

    if let Ok(image) = image_result {
        let cleanup_count = std::rc::Rc::clone(&image_cleanup_count);
        let _guard = EglImageGuard::new(image, move |_| {
            cleanup_count.set(cleanup_count.get() + 1);
        });
    }

    assert_eq!(image_cleanup_count.get(), 0);
}

#[test]
fn texture_creation_failure_destroys_acquired_image_once() {
    let image_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
    let texture_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
    {
        let cleanup_count = std::rc::Rc::clone(&image_cleanup_count);
        let _guard = EglImageGuard::new(fake_egl_image(), move |_| {
            cleanup_count.set(cleanup_count.get() + 1);
        });
        let _texture: Result<(), ()> = Err(());
        assert!(_texture.is_err());
    }

    assert_eq!(image_cleanup_count.get(), 1);
    assert_eq!(texture_cleanup_count.get(), 0);
}

#[test]
fn binding_failure_deletes_texture_and_destroys_image_once() {
    let image_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
    let texture_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
    {
        let cleanup_count = std::rc::Rc::clone(&image_cleanup_count);
        let _guard = EglImageGuard::new(fake_egl_image(), move |_| {
            cleanup_count.set(cleanup_count.get() + 1);
        });
        texture_cleanup_count.set(texture_cleanup_count.get() + 1);
    }

    assert_eq!(image_cleanup_count.get(), 1);
    assert_eq!(texture_cleanup_count.get(), 1);
}

#[test]
fn successful_construction_transfers_image_without_double_cleanup() {
    let image_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
    let texture_cleanup_count = std::rc::Rc::new(std::cell::Cell::new(0));
    let image = {
        let cleanup_count = std::rc::Rc::clone(&image_cleanup_count);
        let guard = EglImageGuard::new(fake_egl_image(), move |_| {
            cleanup_count.set(cleanup_count.get() + 1);
        });
        guard.disarm()
    };

    texture_cleanup_count.set(texture_cleanup_count.get() + 1);
    image_cleanup_count.set(image_cleanup_count.get() + 1);
    let _ = image;

    assert_eq!(image_cleanup_count.get(), 1);
    assert_eq!(texture_cleanup_count.get(), 1);
}

#[test]
fn resource_texture_view_resolves_each_ordinary_texture_kind() {
    let mut resources = RendererResourceState::new(None);
    let frame_color = compositor::ServerFrameColor::ALL[0];
    let solid_color = 0xff12_3456;
    let asset_id = 73;
    let frame_texture = test_texture(1);
    let solid_texture = test_texture(2);
    let asset_texture = test_texture(3);
    let surface_texture = test_texture(4);
    let cursor_texture = test_texture(5);

    resources.test_install_frame_texture(frame_color, frame_texture);
    resources.test_install_decoration_texture(EglDrawLayer::SolidRgba(solid_color), solid_texture);
    resources
        .test_install_decoration_texture(EglDrawLayer::DecorationAsset(asset_id), asset_texture);
    resources.test_install_surface_texture(41, surface_texture);
    resources.test_install_cursor_texture(cursor_texture);

    let view = resources.texture_view();
    assert_eq!(view.texture_for_frame(frame_color), Some(frame_texture));
    assert_eq!(
        view.texture_for_solid_decoration(solid_color),
        Some(solid_texture)
    );
    assert_eq!(
        view.texture_for_decoration_asset(asset_id),
        Some(asset_texture)
    );
    assert_eq!(view.texture_for_surface(41), Some(surface_texture));
    assert_eq!(view.cursor_texture(), Some(cursor_texture));
    assert_eq!(view.texture_for_surface(42), None);
}

#[test]
fn frame_resources_are_created_lazily_and_reused() {
    let mut harness = GlesEffectTestHarness::new(4, 4);
    let color = compositor::ServerFrameColor::ALL[0];
    assert_eq!(
        harness
            .renderer
            .resources
            .texture_view()
            .texture_for_frame(color),
        None
    );

    harness
        .renderer
        .resources
        .ensure_frame_resources(&harness.gl)
        .expect("frame textures create");
    let first = harness
        .renderer
        .resources
        .texture_view()
        .texture_for_frame(color)
        .expect("requested frame texture exists");
    harness
        .renderer
        .resources
        .ensure_frame_resources(&harness.gl)
        .expect("existing frame textures are reused");
    let second = harness
        .renderer
        .resources
        .texture_view()
        .texture_for_frame(color)
        .expect("requested frame texture remains installed");

    assert_eq!(first, second);
}

#[test]
fn decoration_asset_lookup_preserves_premultiplied_rgba_bytes() {
    let mut harness = GlesEffectTestHarness::new(2, 1);
    let asset_id = 73;
    let premultiplied_pixels = [20_u8, 10, 5, 128, 40, 30, 20, 255];
    let texture = harness
        .renderer
        .resources
        .test_create_decoration_texture(
            &harness.gl,
            EglDrawLayer::DecorationAsset(asset_id),
            2,
            1,
            Some(&premultiplied_pixels),
        )
        .expect("decoration asset texture creates");
    let framebuffer = unsafe {
        harness
            .gl
            .create_framebuffer()
            .expect("decoration texture readback framebuffer creates")
    };
    let mut readback = vec![0_u8; premultiplied_pixels.len()];
    let framebuffer_status = unsafe {
        harness
            .gl
            .bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
        harness.gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(texture),
            0,
        );
        let status = harness.gl.check_framebuffer_status(glow::FRAMEBUFFER);
        harness.gl.read_pixels(
            0,
            0,
            2,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut readback)),
        );
        harness.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        harness.gl.delete_framebuffer(framebuffer);
        status
    };

    assert_eq!(framebuffer_status, glow::FRAMEBUFFER_COMPLETE);
    assert_eq!(
        harness
            .renderer
            .resources
            .texture_view()
            .texture_for_decoration_asset(asset_id),
        Some(texture)
    );
    assert_eq!(readback, premultiplied_pixels);
}

#[test]
fn cursor_resource_reuses_matches_and_recreates_after_invalidation() {
    let mut harness = GlesEffectTestHarness::new(4, 4);
    let cursor_image = CompositorCursorImage::from_argb8888(
        vec![0xff00_0000, 0xffff_ffff, 0xff00_ff00, 0xffff_0000],
        2,
        2,
        0,
        0,
    )
    .expect("valid test cursor image");

    harness
        .renderer
        .resources
        .ensure_cursor_resource(&harness.gl, &harness.egl, harness.display, &cursor_image)
        .expect("cursor texture creates");
    let first = harness
        .renderer
        .resources
        .texture_view()
        .cursor_texture()
        .expect("cursor texture exists");
    harness
        .renderer
        .resources
        .ensure_cursor_resource(&harness.gl, &harness.egl, harness.display, &cursor_image)
        .expect("matching cursor texture is reused");
    assert_eq!(
        harness.renderer.resources.texture_view().cursor_texture(),
        Some(first)
    );

    harness.renderer.resources.mark_cursor_stale();
    harness
        .renderer
        .resources
        .ensure_cursor_resource(&harness.gl, &harness.egl, harness.display, &cursor_image)
        .expect("invalidated cursor texture is recreated");
    let second = harness
        .renderer
        .resources
        .texture_view()
        .cursor_texture()
        .expect("replacement cursor texture exists");

    assert_ne!(first, second);
    assert!(!unsafe { harness.gl.is_texture(first) });
}

#[test]
fn scene_texture_sources_keeps_lifecycle_visuals_outside_ordinary_resources() {
    let mut resources = RendererResourceState::new(None);
    let surface_texture = test_texture(11);
    resources.test_install_surface_texture(5, surface_texture);
    let sample = lifecycle_test_lamp_sample(0.5);
    let payload_id = sample.samples[0].payload_id;
    let lifecycle = HashMap::new();
    let sources = SceneTextureSources {
        ordinary: resources.texture_view(),
        lifecycle: &lifecycle,
    };
    let effect_resources = EffectGlResourceCache::new();

    assert_eq!(
        sources.texture_for_layer(EglDrawLayer::Surface(5), &effect_resources),
        Some(surface_texture)
    );
    assert_eq!(
        sources.texture_for_layer(
            EglDrawLayer::LifecycleResolvedVisual(payload_id),
            &effect_resources,
        ),
        None,
        "lifecycle textures resolve through their retained store and effect pool only"
    );
}

#[test]
fn renderer_capture_restores_only_transient_resource_diagnostics() {
    let mut harness = GlesEffectTestHarness::new(4, 4);
    let renderer = &mut harness.renderer;
    let surface_texture = test_texture(21);
    renderer
        .resources
        .test_install_surface_texture(91, surface_texture);
    renderer.resources.test_set_failed_surface_generation(91, 7);
    let resource_identity = renderer
        .resources
        .test_surface_resource_identity(91)
        .expect("surface resource is installed before capture");
    let capture = CaptureRendererState::take(renderer);

    let identity = BufferIdAllocator::default()
        .allocate()
        .expect("cache test buffer identity");
    let fd = std::fs::File::open("/dev/null")
        .expect("/dev/null exists for DMA-BUF identity tests")
        .into();
    let handle = DmabufBufferHandle::new(
        BufferSize::new(1, 1).expect("cache test image size"),
        DrmFormat::Xrgb8888,
        vec![DmabufPlane::new(
            fd,
            DmabufPlaneDescriptor {
                plane_index: 0,
                offset: 0,
                stride: 4,
                modifier: DrmModifier::LINEAR,
            },
        )],
    )
    .expect("cache test DMA-BUF metadata");
    let key = DmabufImageKey::from_handle(identity.id(), &handle);
    renderer.resources.test_insert_cached_dmabuf_texture(
        key,
        test_texture(22),
        identity.downgrade(),
        91,
    );
    renderer.resources.test_set_failed_surface_generation(91, 8);

    capture.restore(renderer);

    assert_eq!(
        renderer.resources.test_failed_surface_generation(91),
        Some(7)
    );
    assert_eq!(renderer.resources.test_dmabuf_cache_entry_count(), 1);
    assert_eq!(
        renderer.resources.test_surface_resource_identity(91),
        Some(resource_identity),
        "capture keeps the installed surface resource object in place"
    );
    assert_eq!(
        renderer.resources.texture_view().texture_for_surface(91),
        Some(surface_texture),
        "capture does not recreate the ordinary surface texture"
    );
}

fn test_texture(value: u32) -> GlTexture {
    glow::NativeTexture(std::num::NonZeroU32::new(value).expect("nonzero test texture"))
}
