use super::*;

pub(crate) struct DumbFramebuffer {
    pub(crate) fd: RawFd,
    pub(crate) handle: u32,
    pub(crate) fb_id: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pitch: u32,
    pub(crate) size: usize,
    pub(crate) mapping: *mut c_void,
    pub(crate) drm_cleanup_armed: bool,
}

impl DumbFramebuffer {
    pub(crate) fn create(file: &fs::File, width: u32, height: u32) -> io::Result<Self> {
        let dumb = drm_ffi::mode::dumbbuffer::create(file.as_fd(), width, height, 32, 0)?;
        let fb = match drm_ffi::mode::add_fb(
            file.as_fd(),
            width,
            height,
            dumb.pitch,
            32,
            24,
            dumb.handle,
        ) {
            Ok(fb) => fb,
            Err(error) => {
                let _ = drm_ffi::mode::dumbbuffer::destroy(file.as_fd(), dumb.handle);
                return Err(error);
            }
        };
        let map = match drm_ffi::mode::dumbbuffer::map(file.as_fd(), dumb.handle, 0, 0) {
            Ok(map) => map,
            Err(error) => {
                let _ = drm_ffi::mode::rm_fb(file.as_fd(), fb.fb_id);
                let _ = drm_ffi::mode::dumbbuffer::destroy(file.as_fd(), dumb.handle);
                return Err(error);
            }
        };
        let mapping = unsafe {
            libc::mmap(
                ptr::null_mut(),
                dumb.size as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                map.offset as libc::off_t,
            )
        };
        if mapping == libc::MAP_FAILED {
            let error = io::Error::last_os_error();
            let _ = drm_ffi::mode::rm_fb(file.as_fd(), fb.fb_id);
            let _ = drm_ffi::mode::dumbbuffer::destroy(file.as_fd(), dumb.handle);
            return Err(error);
        }

        Ok(Self {
            fd: file.as_raw_fd(),
            handle: dumb.handle,
            fb_id: fb.fb_id,
            width,
            height,
            pitch: dumb.pitch,
            size: dumb.size as usize,
            mapping,
            drm_cleanup_armed: true,
        })
    }

    pub(crate) fn paint_server_frame(
        &mut self,
        renderer: &mut NativeFrameRenderer,
        server: &OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
        damage: &NativeOutputDamage,
    ) -> io::Result<NativePaintStats> {
        let total_start = Instant::now();
        let render_start = Instant::now();
        let rendered =
            renderer.render_server_frame(self.width, self.height, server, input_state, cursor_mode);
        let render_us = elapsed_micros(render_start);
        let bytes = unsafe { slice::from_raw_parts_mut(self.mapping.cast::<u8>(), self.size) };
        let copy_start = Instant::now();
        let copy_bytes = copy_argb_frame_to_xrgb_mapping_damage(
            rendered.pixels,
            self.width,
            self.height,
            self.pitch,
            bytes,
            damage.frame_copy_damage_for_scene(rendered.scene_rebuild_kind),
        )?;
        let copy_us = elapsed_micros(copy_start);
        Ok(NativePaintStats {
            backend: NativeScanoutKind::DumbFramebuffer,
            scanout_format: None,
            width: self.width,
            height: self.height,
            bytes: self.size,
            copy_bytes,
            write_bytes: 0,
            gpu_draw_us: 0,
            egl_swap_us: 0,
            shm_upload_bytes: 0,
            dmabuf_imports: 0,
            dmabuf_reuses: 0,
            dmabuf_import_failures: 0,
            dmabuf_cache_entries: 0,
            dmabuf_cache_peak_entries: 0,
            dmabuf_cache_evictions: 0,
            scene_rebuild: rendered.scene_rebuild_kind,
            frame_copy: rendered.frame_copy_kind,
            total_us: elapsed_micros(total_start),
            render_us,
            copy_us,
            write_us: 0,
            gles_repaint: None,
            swap_with_damage_used: false,
        })
    }
}

impl Drop for DumbFramebuffer {
    fn drop(&mut self) {
        let _ = unsafe { libc::munmap(self.mapping, self.size) };
        if !self.drm_cleanup_armed {
            return;
        }
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        let _ = drm_ffi::mode::rm_fb(fd, self.fb_id);
        let _ = drm_ffi::mode::dumbbuffer::destroy(fd, self.handle);
    }
}

pub(crate) fn probe_native_egl_gbm_device(
    bootstrap: &NativeOutputBootstrap,
    perf: &NativePerfLogger,
) {
    if bootstrap.kms_device.is_none() || bootstrap.render_device.is_none() {
        perf.log("native.egl_probe", || {
            vec![
                NativePerfField::str("status", "skipped"),
                NativePerfField::str("reason", "missing_kms_or_render_device"),
            ]
        });
        return;
    }

    let kms_device = bootstrap.kms_device.as_deref().unwrap();
    let render_device = bootstrap.render_device.as_deref().unwrap();

    let egl = match unsafe { egl::DynamicInstance::<egl::EGL1_5>::load_required() } {
        Ok(egl) => egl,
        Err(e) => {
            perf.log("native.egl_probe", || {
                vec![
                    NativePerfField::str("status", "failed"),
                    NativePerfField::str("reason", "egl_load_failed"),
                    NativePerfField::str("error", e.to_string()),
                ]
            });
            return;
        }
    };

    let display_fd = match OpenOptions::new().read(true).write(true).open(kms_device) {
        Ok(fd) => fd,
        Err(e) => {
            perf.log("native.egl_probe", || {
                vec![
                    NativePerfField::str("status", "failed"),
                    NativePerfField::str("reason", "kms_device_open_failed"),
                    NativePerfField::str("device", kms_device.display().to_string()),
                    NativePerfField::str("error", e.to_string()),
                ]
            });
            return;
        }
    };
    let gbm_device = match gbm::Device::new(display_fd) {
        Ok(device) => device,
        Err(e) => {
            perf.log("native.egl_probe", || {
                vec![
                    NativePerfField::str("status", "failed"),
                    NativePerfField::str("reason", "gbm_device_create_failed"),
                    NativePerfField::str("device", kms_device.display().to_string()),
                    NativePerfField::str("error", e.to_string()),
                ]
            });
            return;
        }
    };

    const EGL_PLATFORM_GBM_KHR: egl::Enum = 0x31d7;
    // The GBM platform native display is the gbm_device pointer. Passing the
    // integer DRM fd as a pointer makes the probe succeed/fail for the wrong
    // reason on different EGL stacks.
    let display = match unsafe {
        egl.get_platform_display(
            EGL_PLATFORM_GBM_KHR,
            gbm_device.as_raw_mut() as egl::NativeDisplayType,
            &[egl::ATTRIB_NONE],
        )
    } {
        Ok(display) => display,
        Err(_) => {
            perf.log("native.egl_probe", || {
                vec![
                    NativePerfField::str("status", "failed"),
                    NativePerfField::str("reason", "get_platform_display_failed"),
                ]
            });
            return;
        }
    };

    let (major, minor) = match egl.initialize(display) {
        Ok(version) => version,
        Err(_) => {
            perf.log("native.egl_probe", || {
                vec![
                    NativePerfField::str("status", "failed"),
                    NativePerfField::str("reason", "egl_initialize_failed"),
                ]
            });
            let _ = egl.terminate(display);
            return;
        }
    };

    let vendor_str = egl
        .query_string(Some(display), egl::VENDOR)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let version_str = egl
        .query_string(Some(display), egl::VERSION)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext_str = egl
        .query_string(Some(display), egl::EXTENSIONS)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();

    let has_dmabuf_import = ext_str.contains("EGL_EXT_image_dma_buf_import");
    let has_dmabuf_import_modifiers = ext_str.contains("EGL_EXT_image_dma_buf_import_modifiers");
    let has_native_fence_sync =
        ext_str.contains("EGL_ANDROID_native_fence_sync") || ext_str.contains("EGL_KHR_fence_sync");
    let has_surfaceless = ext_str.contains("EGL_KHR_surfaceless_context");
    let has_pbuffer = ext_str.contains("EGL_KHR_pbuffer_context");

    let config_count = egl.get_config_count(display).unwrap_or_default();
    let has_config = choose_egl_config(&egl, display).is_ok();
    let has_native_xrgb_config =
        choose_native_egl_config(&egl, display, gbm::Format::Xrgb8888 as u32).is_ok();
    let has_native_argb_config =
        choose_native_egl_config(&egl, display, gbm::Format::Argb8888 as u32).is_ok();
    let usage = gbm::BufferObjectFlags::SCANOUT | gbm::BufferObjectFlags::RENDERING;
    let selected_native_format =
        choose_native_egl_gbm_format_config(&egl, display, &gbm_device, usage)
            .ok()
            .map(|format_config| format_config.format as u32);

    let feedback = query_egl_dmabuf_feedback(&egl, display);
    let (main_device_path, main_device) = query_egl_main_device(&egl, display)
        .map(|(path, device)| (Some(path), Some(device)))
        .unwrap_or((None, None));

    let table_format_count = feedback.format_table_formats().len();
    let tranche_format_count = feedback.formats().len();
    let has_nvidia_modifiers = feedback
        .formats()
        .iter()
        .any(|f| (f.modifier.0 & 0xff00_0000_0000_0000) == 0x0300_0000_0000_0000);

    perf.log("native.egl_probe", || {
        vec![
            NativePerfField::str("status", "success"),
            NativePerfField::str("vendor", &vendor_str),
            NativePerfField::str("version", &version_str),
            NativePerfField::str("kms_device", kms_device.display().to_string()),
            NativePerfField::str("render_device", render_device.display().to_string()),
            NativePerfField::u64("major", major as u64),
            NativePerfField::u64("minor", minor as u64),
            NativePerfField::bool("has_config", has_config),
            NativePerfField::bool("has_native_xrgb_config", has_native_xrgb_config),
            NativePerfField::bool("has_native_argb_config", has_native_argb_config),
            NativePerfField::str(
                "selected_native_format",
                selected_native_format
                    .map(native_visual_label)
                    .unwrap_or_else(|| "none".to_string()),
            ),
            NativePerfField::u64("config_count", config_count as u64),
            NativePerfField::bool("dmabuf_import", has_dmabuf_import),
            NativePerfField::bool("dmabuf_import_modifiers", has_dmabuf_import_modifiers),
            NativePerfField::bool("native_fence_sync", has_native_fence_sync),
            NativePerfField::bool("surfaceless_context", has_surfaceless),
            NativePerfField::bool("pbuffer_context", has_pbuffer),
            NativePerfField::u64("table_format_count", table_format_count as u64),
            NativePerfField::u64("tranche_format_count", tranche_format_count as u64),
            NativePerfField::bool("has_nvidia_modifiers", has_nvidia_modifiers),
            NativePerfField::str(
                "main_device",
                main_device
                    .map(|d| d.to_string())
                    .unwrap_or("none".to_string()),
            ),
            NativePerfField::str(
                "main_device_path",
                main_device_path.unwrap_or("none".to_string()),
            ),
        ]
    });

    let _ = egl.terminate(display);
}

#[cfg(test)]
pub(crate) fn copy_argb_frame_to_xrgb_mapping(
    frame: &[u32],
    width: u32,
    height: u32,
    pitch: u32,
    bytes: &mut [u8],
) -> io::Result<()> {
    copy_argb_frame_to_xrgb_mapping_damage(
        frame,
        width,
        height,
        pitch,
        bytes,
        NativeFrameCopyDamage::Full,
    )
    .map(|_| ())
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum NativeFrameCopyDamage<'a> {
    Full,
    Rects(&'a [NativeDamageRect]),
}

pub(crate) fn copy_argb_frame_to_xrgb_mapping_damage(
    frame: &[u32],
    width: u32,
    height: u32,
    pitch: u32,
    bytes: &mut [u8],
    damage: NativeFrameCopyDamage<'_>,
) -> io::Result<usize> {
    let row_bytes = width
        .checked_mul(4)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| io::Error::other("native framebuffer row width overflow"))?;
    let pitch =
        usize::try_from(pitch).map_err(|_| io::Error::other("invalid framebuffer pitch"))?;
    let output_width = width;
    let output_height = height;
    let row_pixels = usize::try_from(width)
        .map_err(|_| io::Error::other("native framebuffer width overflow"))?;
    let height = usize::try_from(height)
        .map_err(|_| io::Error::other("native framebuffer height overflow"))?;
    let pixel_count = row_pixels
        .checked_mul(height)
        .ok_or_else(|| io::Error::other("native framebuffer source overflow"))?;
    if frame.len() < pixel_count {
        return Err(io::Error::other("native framebuffer source is too small"));
    }
    let frame_bytes_len = pixel_count
        .checked_mul(mem::size_of::<u32>())
        .ok_or_else(|| io::Error::other("native framebuffer source byte overflow"))?;
    // XRGB ignores the high byte, so the native ARGB words can be copied as-is.
    let frame_bytes =
        unsafe { slice::from_raw_parts(frame.as_ptr().cast::<u8>(), frame_bytes_len) };

    let full_copy_bytes = row_bytes
        .checked_mul(height)
        .ok_or_else(|| io::Error::other("native framebuffer full copy overflow"))?;
    let full_rect;
    let damage_rects = match damage {
        NativeFrameCopyDamage::Full => {
            full_rect = [NativeDamageRect {
                x: 0,
                y: 0,
                width: output_width,
                height: output_height,
            }];
            &full_rect[..]
        }
        NativeFrameCopyDamage::Rects(rects)
            if damage_rect_copy_bytes(rects, output_width, output_height)? >= full_copy_bytes =>
        {
            full_rect = [NativeDamageRect {
                x: 0,
                y: 0,
                width: output_width,
                height: output_height,
            }];
            &full_rect[..]
        }
        NativeFrameCopyDamage::Rects(rects) => rects,
    };

    let mut copied = 0usize;
    for rect in damage_rects {
        let Some(rect) = rect.clipped_to_output(width, height as u32) else {
            continue;
        };
        let left = usize::try_from(rect.x)
            .map_err(|_| io::Error::other("native framebuffer damage x overflow"))?;
        let top = usize::try_from(rect.y)
            .map_err(|_| io::Error::other("native framebuffer damage y overflow"))?;
        let rect_width = usize::try_from(rect.width)
            .map_err(|_| io::Error::other("native framebuffer damage width overflow"))?;
        let rect_height = usize::try_from(rect.height)
            .map_err(|_| io::Error::other("native framebuffer damage height overflow"))?;
        let rect_row_bytes = rect_width
            .checked_mul(mem::size_of::<u32>())
            .ok_or_else(|| io::Error::other("native framebuffer damage row overflow"))?;

        for y in top..top.saturating_add(rect_height) {
            let dst_start = y
                .checked_mul(pitch)
                .and_then(|value| value.checked_add(left.saturating_mul(mem::size_of::<u32>())))
                .ok_or_else(|| io::Error::other("native framebuffer pitch overflow"))?;
            let dst_end = dst_start
                .checked_add(rect_row_bytes)
                .ok_or_else(|| io::Error::other("native framebuffer row overflow"))?;
            let Some(dst_row) = bytes.get_mut(dst_start..dst_end) else {
                return Err(io::Error::other("native framebuffer mapping is too small"));
            };
            let src_start = y
                .checked_mul(row_bytes)
                .and_then(|value| value.checked_add(left.saturating_mul(mem::size_of::<u32>())))
                .ok_or_else(|| io::Error::other("native framebuffer source overflow"))?;
            let src_end = src_start
                .checked_add(rect_row_bytes)
                .ok_or_else(|| io::Error::other("native framebuffer source overflow"))?;
            dst_row.copy_from_slice(&frame_bytes[src_start..src_end]);
            copied = copied.saturating_add(rect_row_bytes);
        }
    }
    Ok(copied)
}

pub(crate) fn damage_rect_copy_bytes(
    rects: &[NativeDamageRect],
    output_width: u32,
    output_height: u32,
) -> io::Result<usize> {
    let mut bytes = 0usize;
    for rect in rects {
        let Some(rect) = rect.clipped_to_output(output_width, output_height) else {
            continue;
        };
        let rect_width = usize::try_from(rect.width)
            .map_err(|_| io::Error::other("native framebuffer damage width overflow"))?;
        let rect_height = usize::try_from(rect.height)
            .map_err(|_| io::Error::other("native framebuffer damage height overflow"))?;
        let rect_bytes = rect_width
            .checked_mul(rect_height)
            .and_then(|pixels| pixels.checked_mul(mem::size_of::<u32>()))
            .ok_or_else(|| io::Error::other("native framebuffer damage byte overflow"))?;
        bytes = bytes
            .checked_add(rect_bytes)
            .ok_or_else(|| io::Error::other("native framebuffer damage byte overflow"))?;
    }
    Ok(bytes)
}
