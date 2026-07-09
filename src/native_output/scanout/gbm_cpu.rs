use super::*;

pub(crate) struct NativeGbmScanout {
    pub(crate) _device: gbm::Device<OwnedFd>,
    pub(crate) fd: RawFd,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) buffers: Vec<NativeGbmScanoutBuffer>,
    pub(crate) current_index: usize,
    pub(crate) ready_index: Option<usize>,
    pub(crate) pending_index: Option<usize>,
    pub(crate) page_flip: AtomicCommitState,
    pub(crate) backend_generation: u64,
    pub(crate) staging: Vec<u8>,
}

pub(crate) struct NativeGbmScanoutBuffer {
    pub(crate) bo: gbm::BufferObject<()>,
    pub(crate) fb_id: u32,
    pub(crate) pitch: u32,
}

impl NativeGbmScanout {
    pub(crate) fn create(
        kms: &fs::File,
        width: u32,
        height: u32,
        backend_generation: u64,
    ) -> io::Result<Self> {
        let gbm_fd = duplicate_fd_cloexec(kms.as_raw_fd()).map_err(io::Error::from_raw_os_error)?;
        let device = gbm::Device::new(gbm_fd)?;
        let usage = gbm::BufferObjectFlags::SCANOUT
            | gbm::BufferObjectFlags::WRITE
            | gbm::BufferObjectFlags::LINEAR;
        if !device.is_format_supported(gbm::Format::Xrgb8888, usage) {
            return Err(io::Error::other(
                "GBM device does not support writable XRGB8888 scanout buffers",
            ));
        }

        let mut buffers = Vec::new();
        for _ in 0..3 {
            let bo = device.create_buffer_object(width, height, gbm::Format::Xrgb8888, usage)?;
            let fb_id = add_gbm_framebuffer(kms.as_fd(), &bo)?;
            let pitch = bo.stride();
            buffers.push(NativeGbmScanoutBuffer { bo, fb_id, pitch });
        }
        println!(
            "native scanout: GBM write/pageflip buffers ready: {}x{}, {} buffer(s), backend {}",
            width,
            height,
            buffers.len(),
            device.backend_name()
        );
        Ok(Self {
            _device: device,
            fd: kms.as_raw_fd(),
            width,
            height,
            buffers,
            current_index: 0,
            ready_index: None,
            pending_index: None,
            page_flip: AtomicCommitState::default(),
            backend_generation,
            staging: Vec::new(),
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
        let index = self.next_render_index()?;
        let render_start = Instant::now();
        let rendered =
            renderer.render_server_frame(self.width, self.height, server, input_state, cursor_mode);
        let render_us = elapsed_micros(render_start);
        let buffer = &mut self.buffers[index];
        let byte_len = buffer
            .pitch
            .checked_mul(self.height)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| io::Error::other("GBM scanout buffer size overflow"))?;
        let copy_start = Instant::now();
        self.staging.resize(byte_len, 0);
        let copy_bytes = copy_argb_frame_to_xrgb_mapping_damage(
            rendered.pixels,
            self.width,
            self.height,
            buffer.pitch,
            &mut self.staging,
            damage.frame_copy_damage_for_scene(rendered.scene_rebuild_kind),
        )?;
        let copy_us = elapsed_micros(copy_start);
        let write_start = Instant::now();
        buffer.bo.write(&self.staging)?;
        let write_us = elapsed_micros(write_start);
        self.ready_index = Some(index);
        Ok(NativePaintStats {
            backend: NativeScanoutKind::GbmCpuWritePageFlip,
            scanout_format: None,
            width: self.width,
            height: self.height,
            bytes: byte_len,
            copy_bytes,
            write_bytes: byte_len,
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
            write_us,
            gles_repaint: None,
            swap_with_damage_used: false,
        })
    }

    pub(crate) fn fb_id(&self) -> u32 {
        self.ready_index
            .map(|index| self.buffers[index].fb_id)
            .unwrap_or_else(|| self.buffers[self.current_index].fb_id)
    }

    pub(crate) fn finish_initial_scanout(&mut self) {
        if let Some(index) = self.ready_index.take() {
            self.current_index = index;
        }
    }

    pub(crate) fn present(&mut self, kms: &KmsBackendSelection) -> io::Result<Option<u64>> {
        if self.page_flip.is_pending() {
            return Ok(None);
        }
        let Some(index) = self.ready_index.take() else {
            return Ok(None);
        };
        if index == self.current_index {
            return Ok(None);
        }
        let framebuffer = FramebufferId::new(self.buffers[index].fb_id)
            .ok_or_else(|| io::Error::other("ready CPU GBM framebuffer ID is zero"))?;
        let token = PageFlipToken::new(allocate_native_page_flip_token())
            .expect("native pageflip allocator never returns zero");
        self.page_flip
            .begin(token, framebuffer, self.backend_generation, Instant::now())
            .map_err(io::Error::other)?;
        match kms.submit_flip(framebuffer, token) {
            Ok(()) => {
                self.pending_index = Some(index);
                Ok(Some(token.get()))
            }
            Err(error) => {
                self.page_flip.submission_failed(token);
                self.ready_index = Some(index);
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
                        if let Some(index) = self.pending_index.take() {
                            self.current_index = index;
                        }
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

    pub(crate) fn ready_frame_queued(&self) -> bool {
        self.ready_index.is_some()
    }

    pub(crate) fn next_render_index(&self) -> io::Result<usize> {
        if let Some(index) = self.ready_index {
            return Ok(index);
        }
        self.buffers
            .iter()
            .enumerate()
            .map(|(index, _)| index)
            .find(|index| {
                Some(*index) != self.pending_index
                    && Some(*index) != self.ready_index
                    && *index != self.current_index
            })
            .ok_or_else(|| io::Error::other("no free GBM scanout buffer is available"))
    }
}

impl Drop for NativeGbmScanout {
    fn drop(&mut self) {
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        for buffer in &self.buffers {
            let _ = drm_ffi::mode::rm_fb(fd, buffer.fb_id);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct NativeGbmFramebufferMetadata {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: u32,
    pub(crate) handles: [u32; 4],
    pub(crate) pitches: [u32; 4],
    pub(crate) offsets: [u32; 4],
    pub(crate) modifiers: [u64; 4],
    pub(crate) flags: u32,
}

impl NativeGbmFramebufferMetadata {
    pub(crate) fn from_bo(bo: &gbm::BufferObject<()>) -> Self {
        let mut handles = [0; 4];
        let mut pitches = [0; 4];
        let mut offsets = [0; 4];
        let mut modifiers = [0; 4];
        let plane_count = bo.plane_count().clamp(1, 4);
        let modifier = u64::from(bo.modifier());
        for plane in 0..plane_count {
            let index = plane as usize;
            handles[index] = unsafe { bo.handle_for_plane(plane as i32).u32_ };
            if handles[index] == 0 {
                handles[index] = unsafe { bo.handle().u32_ };
            }
            pitches[index] = bo.stride_for_plane(plane as i32);
            if pitches[index] == 0 {
                pitches[index] = bo.stride();
            }
            offsets[index] = bo.offset(plane as i32);
            modifiers[index] = modifier;
        }
        let flags = if bo.modifier() == gbm::Modifier::Invalid {
            0
        } else {
            drm_sys::DRM_MODE_FB_MODIFIERS
        };
        Self {
            width: bo.width(),
            height: bo.height(),
            format: bo.format() as u32,
            handles,
            pitches,
            offsets,
            modifiers,
            flags,
        }
    }
}

pub(crate) fn add_gbm_framebuffer(
    fd: BorrowedFd<'_>,
    bo: &gbm::BufferObject<()>,
) -> io::Result<u32> {
    let metadata = NativeGbmFramebufferMetadata::from_bo(bo);
    drm_ffi::mode::add_fb2(
        fd,
        metadata.width,
        metadata.height,
        metadata.format,
        &metadata.handles,
        &metadata.pitches,
        &metadata.offsets,
        &metadata.modifiers,
        metadata.flags,
    )
    .map(|framebuffer| framebuffer.fb_id)
}

pub(crate) fn set_fd_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
