use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativePageFlipBuffers<T> {
    pub(crate) current: Option<T>,
    pub(crate) ready: Option<T>,
    pub(crate) pending: Option<T>,
}

impl<T> Default for NativePageFlipBuffers<T> {
    fn default() -> Self {
        Self {
            current: None,
            ready: None,
            pending: None,
        }
    }
}

impl<T> NativePageFlipBuffers<T> {
    pub(crate) fn set_ready(&mut self, buffer: T) {
        self.ready = Some(buffer);
    }

    pub(crate) fn ready_or_current(&self) -> Option<&T> {
        self.ready.as_ref().or(self.current.as_ref())
    }

    pub(crate) fn finish_initial_scanout(&mut self) {
        if let Some(buffer) = self.ready.take() {
            self.current = Some(buffer);
        }
    }

    pub(crate) fn take_ready(&mut self) -> Option<T> {
        self.ready.take()
    }

    pub(crate) fn restore_ready(&mut self, buffer: T) {
        self.ready = Some(buffer);
    }

    pub(crate) fn set_pending(&mut self, buffer: T) {
        self.pending = Some(buffer);
    }

    pub(crate) fn complete_page_flip(&mut self) -> bool {
        let Some(buffer) = self.pending.take() else {
            return false;
        };
        self.current = Some(buffer);
        true
    }
}

pub(crate) struct NativeEglGbmScanout {
    pub(crate) _device: gbm::Device<OwnedFd>,
    pub(crate) surface: gbm::Surface<()>,
    pub(crate) format: gbm::Format,
    pub(crate) fd: RawFd,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) egl: EglInstance,
    pub(crate) egl_display: egl::Display,
    pub(crate) egl_context: egl::Context,
    pub(crate) egl_surface: egl::Surface,
    pub(crate) scene: GlesSceneRenderer,
    pub(crate) swap_buffers_with_damage: Option<EglSwapBuffersWithDamage>,
    pub(crate) dmabuf_feedback: EglGlesDmabufFeedback,
    pub(crate) dmabuf_main_device: Option<u64>,
    pub(crate) dmabuf_main_device_path: Option<String>,
    pub(crate) framebuffer_cache: NativeGbmFramebufferCache,
    pub(crate) buffers: NativePageFlipBuffers<NativePresentedGbmBuffer>,
    pub(crate) page_flip: AtomicCommitState,
    pub(crate) backend_generation: u64,
}

// A locked GBM front buffer must stay alive while KMS may scan it out. Ready is
// not on KMS yet, pending is waiting for the DRM event, and current is the last
// completed pageflip/modeset buffer. Dropping this value releases the GBM BO
// back to the surface, so transitions are driven only by modeset/pageflip state.
pub(crate) struct NativePresentedGbmBuffer {
    pub(crate) _bo: gbm::BufferObject<()>,
    pub(crate) fb_id: u32,
}

pub(crate) struct NativeEglGbmFormatConfig {
    pub(crate) format: gbm::Format,
    pub(crate) config: egl::Config,
}

pub(crate) fn native_egl_gbm_format_candidates() -> [gbm::Format; 2] {
    [gbm::Format::Xrgb8888, gbm::Format::Argb8888]
}

pub(crate) fn choose_native_egl_gbm_format_config<T: AsFd>(
    egl: &EglInstance,
    egl_display: egl::Display,
    device: &gbm::Device<T>,
    usage: gbm::BufferObjectFlags,
) -> io::Result<NativeEglGbmFormatConfig> {
    let mut unsupported = Vec::new();
    let mut egl_errors = Vec::new();
    for format in native_egl_gbm_format_candidates() {
        let format_label = native_visual_label(format as u32);
        if !device.is_format_supported(format, usage) {
            unsupported.push(format_label);
            continue;
        }
        match choose_native_egl_config(egl, egl_display, format as u32) {
            Ok(config) => return Ok(NativeEglGbmFormatConfig { format, config }),
            Err(error) => egl_errors.push(format!("{format_label}: {error}")),
        }
    }
    let unsupported = if unsupported.is_empty() {
        "none".to_string()
    } else {
        unsupported.join(", ")
    };
    let egl_errors = if egl_errors.is_empty() {
        "none".to_string()
    } else {
        egl_errors.join("; ")
    };
    Err(io::Error::other(format!(
        "GBM/EGL has no compatible native scanout format; unsupported_by_gbm={unsupported}; egl_errors={egl_errors}"
    )))
}

#[derive(Debug, Default)]
pub(crate) struct NativeGbmFramebufferCache {
    pub(crate) entries: HashMap<NativeGbmFramebufferMetadata, u32>,
}

impl NativeGbmFramebufferCache {
    pub(crate) fn fb_id_for(
        &mut self,
        fd: BorrowedFd<'_>,
        bo: &gbm::BufferObject<()>,
    ) -> io::Result<u32> {
        let metadata = NativeGbmFramebufferMetadata::from_bo(bo);
        if let Some(fb_id) = self.entries.get(&metadata) {
            return Ok(*fb_id);
        }
        let fb_id = add_gbm_framebuffer(fd, bo)?;
        self.entries.insert(metadata, fb_id);
        Ok(fb_id)
    }

    pub(crate) fn clear(&mut self, fd: BorrowedFd<'_>) {
        for (_, fb_id) in self.entries.drain() {
            let _ = drm_ffi::mode::rm_fb(fd, fb_id);
        }
    }
}

impl NativeEglGbmScanout {
    pub(crate) fn create(
        kms: &fs::File,
        width: u32,
        height: u32,
        backend_generation: u64,
    ) -> io::Result<Self> {
        let gbm_fd = duplicate_fd_cloexec(kms.as_raw_fd()).map_err(io::Error::from_raw_os_error)?;
        let device = gbm::Device::new(gbm_fd)?;
        let usage = gbm::BufferObjectFlags::SCANOUT | gbm::BufferObjectFlags::RENDERING;

        let egl = unsafe { EglInstance::load_required() }.map_err(native_egl_io_error)?;
        const EGL_PLATFORM_GBM_KHR: egl::Enum = 0x31d7;
        // EGL_PLATFORM_GBM_KHR requires a gbm_device pointer, not a DRM fd cast
        // to a pointer. The GBM device is kept alive by NativeEglGbmScanout.
        let egl_display = match unsafe {
            egl.get_platform_display(
                EGL_PLATFORM_GBM_KHR,
                device.as_raw_mut() as egl::NativeDisplayType,
                &[egl::ATTRIB_NONE],
            )
        } {
            Ok(display) => display,
            Err(error) => return Err(native_egl_io_error(error)),
        };
        if let Err(error) = egl.initialize(egl_display) {
            let _ = egl.terminate(egl_display);
            return Err(native_egl_io_error(error));
        }
        if let Err(error) = egl.bind_api(egl::OPENGL_ES_API) {
            let _ = egl.terminate(egl_display);
            return Err(native_egl_io_error(error));
        }
        let format_config =
            match choose_native_egl_gbm_format_config(&egl, egl_display, &device, usage) {
                Ok(format_config) => format_config,
                Err(error) => {
                    let _ = egl.terminate(egl_display);
                    return Err(error);
                }
            };
        let surface = match device.create_surface(width, height, format_config.format, usage) {
            Ok(surface) => surface,
            Err(error) => {
                let _ = egl.terminate(egl_display);
                return Err(error);
            }
        };
        let egl_config = format_config.config;
        let egl_context = match create_gles_context(&egl, egl_display, egl_config) {
            Ok(context) => context,
            Err(error) => {
                let _ = egl.terminate(egl_display);
                return Err(native_egl_io_error(error));
            }
        };
        let egl_surface = match unsafe {
            egl.create_platform_window_surface(
                egl_display,
                egl_config,
                surface.as_raw_mut() as egl::NativeWindowType,
                &[egl::ATTRIB_NONE],
            )
        } {
            Ok(surface) => surface,
            Err(error) => {
                let _ = egl.destroy_context(egl_display, egl_context);
                let _ = egl.terminate(egl_display);
                return Err(native_egl_io_error(error));
            }
        };
        if let Err(error) = egl.make_current(
            egl_display,
            Some(egl_surface),
            Some(egl_surface),
            Some(egl_context),
        ) {
            let _ = egl.destroy_surface(egl_display, egl_surface);
            let _ = egl.destroy_context(egl_display, egl_context);
            let _ = egl.terminate(egl_display);
            return Err(native_egl_io_error(error));
        }
        if let Err(error) = egl.swap_interval(egl_display, 1) {
            eprintln!("native EGL/GBM: EGL swap interval unavailable: {error}");
        }

        let egl_image_target_texture_2d: Option<GlEglImageTargetTexture2DOes> =
            load_egl_image_target_texture_2d(&egl).or_else(|| {
                eprintln!(
                    "native EGL/GBM: GL_OES_EGL_image entry point unavailable; dmabuf surfaces will be skipped"
                );
                None
            });
        let swap_buffers_with_damage = load_swap_buffers_with_damage(&egl, egl_display);
        let scene = match GlesSceneRenderer::new_current(
            &egl,
            width,
            height,
            egl_image_target_texture_2d,
            detect_partial_repaint_capabilities(
                &egl,
                egl_display,
                swap_buffers_with_damage.is_some(),
            ),
        ) {
            Ok(scene) => scene,
            Err(error) => {
                let _ = egl.make_current(egl_display, None, None, None);
                let _ = egl.destroy_surface(egl_display, egl_surface);
                let _ = egl.destroy_context(egl_display, egl_context);
                let _ = egl.terminate(egl_display);
                return Err(native_egl_io_error(error));
            }
        };
        let dmabuf_feedback = query_egl_dmabuf_feedback(&egl, egl_display);
        let (dmabuf_main_device_path, dmabuf_main_device) =
            match query_egl_main_device(&egl, egl_display) {
                Some((path, main_device)) => (Some(path), Some(main_device)),
                None => (None, None),
            };
        let vendor = egl
            .query_string(Some(egl_display), egl::VENDOR)
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "unknown".to_string());
        let version = egl
            .query_string(Some(egl_display), egl::VERSION)
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "unknown".to_string());
        let gl_info = scene.renderer_info();
        println!(
            "native EGL/GBM GLES3 renderer active: EGL {vendor} {version}; GL {} {} ({}) on {} format {}",
            gl_info.vendor,
            gl_info.renderer,
            gl_info.version,
            device.backend_name(),
            native_visual_label(format_config.format as u32)
        );

        Ok(Self {
            _device: device,
            surface,
            format: format_config.format,
            fd: kms.as_raw_fd(),
            width,
            height,
            egl,
            egl_display,
            egl_context,
            egl_surface,
            scene,
            swap_buffers_with_damage,
            dmabuf_feedback,
            dmabuf_main_device,
            dmabuf_main_device_path,
            framebuffer_cache: NativeGbmFramebufferCache::default(),
            buffers: NativePageFlipBuffers::default(),
            page_flip: AtomicCommitState::default(),
            backend_generation,
        })
    }

    pub(crate) fn paint_server_frame(
        &mut self,
        renderer: &mut NativeFrameRenderer,
        server: &OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
        damage: &NativeOutputDamage,
    ) -> io::Result<NativePaintOutcome> {
        if !self.surface.has_free_buffers() {
            return Err(io::Error::other(
                "native EGL/GBM surface has no free buffers",
            ));
        }

        let total_start = Instant::now();
        self.egl
            .make_current(
                self.egl_display,
                Some(self.egl_surface),
                Some(self.egl_surface),
                Some(self.egl_context),
            )
            .map_err(native_egl_io_error)?;
        let request = renderer.egl_scene_draw_request(
            self.width,
            self.height,
            server,
            input_state,
            cursor_mode,
            Some(damage.as_renderer_damage(self.width, self.height)),
        );
        let draw_start = Instant::now();
        let outcome = self
            .scene
            .draw_scene(&self.egl, self.egl_display, self.egl_surface, request)
            .map_err(native_egl_io_error)?;
        let draw_us = elapsed_micros(draw_start);
        let EglFrameOutcome::Rendered {
            plan: output_damage,
            ..
        } = outcome
        else {
            let EglFrameOutcome::Skipped { stats, .. } = outcome else {
                unreachable!();
            };
            return Ok(NativePaintOutcome::Skipped(native_egl_gbm_paint_stats(
                self.format as u32,
                self.width,
                self.height,
                draw_us,
                0,
                elapsed_micros(total_start),
                stats,
                false,
            )));
        };
        let swap_with_damage_used = self.swap_buffers_with_damage.is_some()
            && output_damage
                .swap_damage()
                .to_egl_rects(self.width, self.height)
                .is_some();
        let swap_start = Instant::now();
        egl_swap_buffers_with_damage(
            &self.egl,
            self.egl_display,
            self.egl_surface,
            self.swap_buffers_with_damage,
            output_damage.swap_damage(),
            (self.width, self.height),
        )
        .map_err(|error| {
            self.scene.frame_swap_failed();
            native_egl_io_error(error)
        })?;
        self.scene.frame_presented(&output_damage);
        let scene_stats = self.scene.last_frame_stats();
        let swap_us = elapsed_micros(swap_start);
        let bo = unsafe { self.surface.lock_front_buffer() }.map_err(|error| {
            io::Error::other(format!("failed to lock GBM front buffer: {error}"))
        })?;
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        let fb_id = self.framebuffer_cache.fb_id_for(fd, &bo)?;
        self.buffers
            .set_ready(NativePresentedGbmBuffer { _bo: bo, fb_id });
        Ok(NativePaintOutcome::Rendered(native_egl_gbm_paint_stats(
            self.format as u32,
            self.width,
            self.height,
            draw_us,
            swap_us,
            elapsed_micros(total_start),
            scene_stats,
            swap_with_damage_used,
        )))
    }

    pub(crate) fn fb_id(&self) -> u32 {
        self.buffers
            .ready_or_current()
            .map(|buffer| buffer.fb_id)
            .unwrap_or(0)
    }

    pub(crate) fn finish_initial_scanout(&mut self) {
        self.buffers.finish_initial_scanout();
    }

    pub(crate) fn present(&mut self, kms: &KmsBackendSelection) -> io::Result<Option<u64>> {
        if self.page_flip.is_pending() {
            return Ok(None);
        }
        let Some(buffer) = self.buffers.take_ready() else {
            return Ok(None);
        };
        let framebuffer = FramebufferId::new(buffer.fb_id)
            .ok_or_else(|| io::Error::other("ready EGL/GBM framebuffer ID is zero"))?;
        let token = PageFlipToken::new(allocate_native_page_flip_token())
            .expect("native pageflip allocator never returns zero");
        self.page_flip
            .begin(token, framebuffer, self.backend_generation, Instant::now())
            .map_err(io::Error::other)?;
        match kms.submit_flip(framebuffer, token) {
            Ok(()) => {
                self.buffers.set_pending(buffer);
                Ok(Some(token.get()))
            }
            Err(error) => {
                self.page_flip.submission_failed(token);
                self.buffers.restore_ready(buffer);
                Err(io::Error::other(error))
            }
        }
    }

    pub(crate) fn drain_page_flip_events(&mut self, fd: RawFd) -> io::Result<NativePageFlipDrain> {
        let mut drain = NativePageFlipDrain::default();
        for event in drain_drm_page_flip_events(fd)? {
            let expected = self.page_flip.pending_token().map(PageFlipToken::get);
            let Some(token) = PageFlipToken::new(event.user_data) else {
                drain.stale_events = drain.stale_events.saturating_add(1);
                drain.last_stale_token = Some(event.user_data);
                continue;
            };
            match self.page_flip.complete(token, self.backend_generation) {
                AtomicCompletion::Completed { .. } => {
                    if drain.completion.is_none() {
                        self.buffers.complete_page_flip();
                        drain.completion = Some(event);
                    } else {
                        drain.stale_events = drain.stale_events.saturating_add(1);
                    }
                }
                AtomicCompletion::Mismatched => {
                    drain.mismatched_events = drain.mismatched_events.saturating_add(1);
                    drain.last_mismatch = expected.map(|expected| (expected, event.user_data));
                }
                AtomicCompletion::Stale | AtomicCompletion::StaleGeneration => {
                    drain.stale_events = drain.stale_events.saturating_add(1);
                    drain.last_stale_token = Some(event.user_data);
                }
            }
        }
        Ok(drain)
    }

    pub(crate) fn page_flip_pending(&self) -> bool {
        self.page_flip.is_pending()
    }
}

impl Drop for NativeEglGbmScanout {
    fn drop(&mut self) {
        // GL textures/EGLImages are destroyed while the context is current.
        // DRM framebuffer IDs are removed before the locked GBM BO guards drop.
        let _ = self.egl.make_current(
            self.egl_display,
            Some(self.egl_surface),
            Some(self.egl_surface),
            Some(self.egl_context),
        );
        self.scene.destroy(&self.egl, self.egl_display);
        let _ = self.egl.make_current(self.egl_display, None, None, None);
        let _ = self.egl.destroy_surface(self.egl_display, self.egl_surface);
        let _ = self.egl.destroy_context(self.egl_display, self.egl_context);
        let _ = self.egl.terminate(self.egl_display);
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        self.framebuffer_cache.clear(fd);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn native_egl_gbm_paint_stats(
    scanout_format: u32,
    width: u32,
    height: u32,
    draw_us: u64,
    swap_us: u64,
    total_us: u64,
    scene_stats: GlesSceneFrameStats,
    swap_with_damage_used: bool,
) -> NativePaintStats {
    NativePaintStats {
        backend: NativeScanoutKind::NativeEglGbm,
        scanout_format: Some(scanout_format),
        width,
        height,
        bytes: 0,
        copy_bytes: 0,
        write_bytes: 0,
        gpu_draw_us: draw_us,
        egl_swap_us: swap_us,
        shm_upload_bytes: scene_stats.shm_upload_bytes,
        dmabuf_imports: scene_stats.dmabuf_imports,
        dmabuf_reuses: scene_stats.dmabuf_reuses,
        dmabuf_import_failures: scene_stats.dmabuf_import_failures,
        dmabuf_cache_entries: scene_stats.dmabuf_cache_entries,
        dmabuf_cache_peak_entries: scene_stats.dmabuf_cache_peak_entries,
        dmabuf_cache_evictions: scene_stats.dmabuf_cache_evictions,
        scene_rebuild: if scene_stats.scene_rebuilt {
            DesktopSceneRebuildKind::Full
        } else {
            DesktopSceneRebuildKind::None
        },
        frame_copy: DesktopFrameCopyKind::None,
        total_us,
        render_us: draw_us,
        copy_us: 0,
        write_us: 0,
        gles_repaint: Some(scene_stats),
        swap_with_damage_used,
    }
}

pub(crate) fn native_egl_io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}
